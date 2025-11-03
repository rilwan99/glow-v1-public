// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Copyright (C) 2024 A1 XYZ, INC.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use anyhow::{bail, Context, Result};
use glow_program_common::oracle::TokenPriceOracle;
use glow_program_common::token_change::TokenChange;
use glow_solana_client::network::NetworkKind;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::VersionedTransaction;

use anchor_lang::AccountDeserialize;

use glow_margin::{AccountFeatureFlags, LiquidationState, MarginAccount, TokenConfig, TokenKind};
use glow_margin_pool::MarginPool;
use glow_simulation::solana_rpc_api::SolanaRpcClient;

use crate::cat;
use crate::get_state::{get_margin_account, get_position_config, get_token_metadata};
use crate::refresh::deposit::refresh_deposit_positions;
use crate::refresh::pool::{
    refresh_all_pool_positions, refresh_all_pool_positions_underlying_to_tx,
};
use crate::refresh::position_refresher::{HasMarginAccountAddress, HasRpc, PositionRefresher};
use crate::solana::pubkey::OrAta;
use crate::solana::transaction::WithSigner;
use crate::util::data::Join;
use crate::{
    ix_builder::*,
    solana::{
        keypair::{clone, KeypairExt},
        transaction::{SendTransactionBuilder, TransactionBuilder},
    },
};

use super::invoke_pool::PoolTargetPosition;
use super::MarginInvokeContext;

/// [Transaction] builder for a margin account, which supports invoking adapter
/// actions signed as the margin account.
/// Actions are invoked through `adapter_invoke_ix` depending on their context.
///
/// Both margin accounts and liquidators can use this builder, and it will invoke
/// the correct `adapter_invoke_ix`.
pub struct MarginTxBuilder {
    rpc: Arc<dyn SolanaRpcClient>,
    /// builds the instructions for margin without any rpc interaction or
    /// knowledge of other programs
    pub ix: MarginIxBuilder,
    config_ix: MarginConfigIxBuilder,
    signer: Option<Keypair>,
    is_liquidator: bool,
    network_kind: NetworkKind,
}

impl Clone for MarginTxBuilder {
    fn clone(&self) -> Self {
        Self {
            rpc: self.rpc.clone(),
            ix: self.ix.clone(),
            config_ix: self.config_ix.clone(),
            signer: self
                .signer
                .as_ref()
                .map(|kp| Keypair::from_bytes(&kp.to_bytes()).unwrap()),
            is_liquidator: self.is_liquidator,
            network_kind: self.network_kind,
        }
    }
}

#[async_trait]
impl PositionRefresher<MarginAccount> for MarginTxBuilder {
    async fn refresh_positions(
        &self,
        margin_account: &MarginAccount,
    ) -> Result<Vec<(TransactionBuilder, TokenPriceOracle)>> {
        Ok(cat![
            refresh_all_pool_positions(&self.rpc, margin_account).await?,
            refresh_deposit_positions(&self.rpc, margin_account).await?,
        ])
    }
}
impl HasRpc for MarginTxBuilder {
    fn rpc(&self) -> Arc<dyn SolanaRpcClient> {
        self.rpc.clone()
    }
}
impl HasMarginAccountAddress for MarginTxBuilder {
    fn margin_account_address(&self) -> Pubkey {
        self.ix.address
    }
}

impl MarginTxBuilder {
    /// Create a [MarginTxBuilder] for an ordinary user. Liquidators should use
    /// `Self::new_liquidator`.
    pub fn new(
        rpc: Arc<dyn SolanaRpcClient>,
        signer: Option<Keypair>,
        owner: Pubkey,
        seed: u16,
        airspace: Pubkey,
        network_kind: NetworkKind,
    ) -> MarginTxBuilder {
        let mut ix = MarginIxBuilder::new(airspace, owner, seed).with_payer(rpc.payer().pubkey());
        if let Some(signer) = signer.as_ref() {
            ix = ix.with_authority(signer.pubkey());
        }
        let config_ix = MarginConfigIxBuilder::new(
            AirspaceDetails::from_address(airspace),
            rpc.payer().pubkey(),
        );

        Self {
            rpc,
            ix,
            config_ix,
            signer,
            is_liquidator: false,
            network_kind,
        }
    }

    /// Create a new [MarginTxBuilder] for a liquidator. Sets the liquidator
    /// as the authority when interacting with the margin program.
    ///
    /// A liquidator is almost always the payer of the transaction,
    /// their pubkey would be the same as `rpc.payer()`, however we explicitly
    /// supply it to support cases where the liquidator is not the fee payer.
    pub fn new_liquidator(
        rpc: Arc<dyn SolanaRpcClient>,
        liquidator: Keypair,
        airspace: Pubkey,
        owner: Pubkey,
        seed: u16,
        network_kind: NetworkKind,
    ) -> MarginTxBuilder {
        let ix = MarginIxBuilder::new(airspace, owner, seed)
            .with_payer(rpc.payer().pubkey())
            .with_authority(liquidator.pubkey());
        let config_ix = MarginConfigIxBuilder::new(
            AirspaceDetails::from_address(airspace),
            rpc.payer().pubkey(),
        );

        Self {
            rpc,
            ix,
            config_ix,
            signer: Some(liquidator),
            is_liquidator: true,
            network_kind,
        }
    }

    /// returns None if there is no signer.
    pub fn invoke_ctx(&self) -> MarginInvokeContext {
        MarginInvokeContext {
            airspace: self.airspace(),
            margin_account: *self.address(),
            authority: MarginActionAuthority::AccountAuthority.resolve(&self.ix),
            is_liquidator: self.is_liquidator,
        }
    }

    /// whether the current builder is for a liquidator
    pub fn is_liquidator(&self) -> bool {
        self.is_liquidator
    }

    /// Creates a new Self for actions on the same margin account, but
    /// authorized by provided liquidator.
    pub fn liquidator(&self, liquidator: Keypair) -> Self {
        Self {
            rpc: self.rpc.clone(),
            ix: self.ix.clone().with_authority(liquidator.pubkey()),
            config_ix: self.config_ix.clone(),
            signer: Some(liquidator),
            is_liquidator: true,
            network_kind: self.network_kind,
        }
    }

    /// Creates a variant of the builder that has a signer other than the payer.
    pub fn with_signer(mut self, signer: Keypair) -> Self {
        self.ix = self.ix.with_authority(signer.pubkey());
        self.signer = Some(signer);

        self
    }

    async fn create_transaction(
        &self,
        instructions: &[Instruction],
    ) -> Result<VersionedTransaction> {
        let signers = self.signer.as_ref().map(|s| vec![s]).unwrap_or_default();

        self.rpc.create_transaction(&signers, instructions).await
    }

    fn create_transaction_builder(&self, instructions: &[Instruction]) -> TransactionBuilder {
        let signers = self
            .signer
            .as_ref()
            .map(|s| vec![s.clone()])
            .unwrap_or_default();

        TransactionBuilder {
            signers,
            instructions: instructions.to_vec(),
        }
    }

    async fn create_unsigned_transaction(
        &self,
        instructions: &[Instruction],
    ) -> Result<VersionedTransaction> {
        self.rpc.create_transaction(&[], instructions).await
    }

    /// The address of the transaction signer
    pub fn signer(&self) -> Pubkey {
        self.signer.as_ref().unwrap().pubkey()
    }

    /// The address of the transaction signer
    fn signers(&self) -> Vec<Keypair> {
        match &self.signer {
            Some(s) => vec![clone(s)],
            None => vec![],
        }
    }

    /// The owner of the margin account
    pub fn owner(&self) -> &Pubkey {
        &self.ix.owner
    }

    /// The address of the margin account
    pub fn address(&self) -> &Pubkey {
        &self.ix.address
    }

    /// The seed of the margin account
    pub fn seed(&self) -> u16 {
        self.ix.seed
    }

    /// The address of the associated airspace
    pub fn airspace(&self) -> Pubkey {
        self.ix.airspace_details.address
    }

    /// Transaction to create a new margin account for the user
    pub async fn create_account(
        &self,
        features: AccountFeatureFlags,
    ) -> Result<VersionedTransaction> {
        self.create_transaction(&[self.ix.create_account(features)])
            .await
    }

    /// Transaction to close the user's margin account
    pub async fn close_account(&self) -> Result<VersionedTransaction> {
        self.create_transaction(&[self.ix.close_account()]).await
    }

    /// Transaction to create an address lookup registry account
    pub async fn init_lookup_registry(&self) -> Result<VersionedTransaction> {
        self.create_transaction(&[self.ix.init_lookup_registry()])
            .await
    }

    /// Transaction to create a lookup table account
    pub async fn create_lookup_table(&self) -> Result<(VersionedTransaction, Pubkey)> {
        let recent_slot = self
            .rpc
            .get_slot(Some(CommitmentConfig::finalized()))
            .await?
            - 2; // Subtracting works in the program-test runtime, it should be fine.
        let (ix, lookup_table) = self.ix.create_lookup_table(recent_slot);
        let tx = self.create_transaction(&[ix]).await?;

        Ok((tx, lookup_table))
    }

    /// Transaction to append accounts to a lookup table account
    pub async fn append_to_lookup_table(
        &self,
        lookup_table: Pubkey,
        addresses: &[Pubkey],
    ) -> Result<VersionedTransaction> {
        self.create_transaction(&[self.ix.append_to_lookup_table(lookup_table, addresses)])
            .await
    }

    /// Transaction to close the user's margin position accounts for a token mint.
    ///
    /// Both the deposit and loan position should be empty.
    /// Use [Self::close_empty_positions] to close all empty positions.
    pub async fn close_pool_positions(&self, token_mint: MintInfo) -> Result<VersionedTransaction> {
        let pool = MarginPoolIxBuilder::new(self.airspace(), token_mint);
        let deposit_account = self.ix.get_token_account_address(&pool.deposit_note_mint);
        let instructions = vec![
            self.ix
                .close_position(pool.pool_deposit_mint_info(), deposit_account),
            self.adapter_invoke_ix(pool.close_loan(*self.address(), self.ix.payer()), None),
        ];
        self.create_transaction(&instructions).await
    }

    /// Transaction to close the user's margin position account for a token mint and position king.
    ///
    /// The position should be empty.
    pub async fn close_pool_position(
        &self,
        token_mint: MintInfo,
        kind: TokenKind,
    ) -> Result<VersionedTransaction> {
        let pool = MarginPoolIxBuilder::new(self.airspace(), token_mint);
        let ix = match kind {
            TokenKind::Collateral => self.ix.close_position(
                pool.pool_deposit_mint_info(),
                self.ix.get_token_account_address(&pool.deposit_note_mint),
            ),
            TokenKind::Claim => {
                self.adapter_invoke_ix(pool.close_loan(*self.address(), self.ix.payer()), None)
            }
            TokenKind::AdapterCollateral => panic!("pools do not issue AdapterCollateral"),
        };

        self.create_transaction(&[ix]).await
    }

    /// Transaction to close the user's empty position accounts.
    pub async fn close_empty_positions(
        &self,
        loan_to_token: &HashMap<Pubkey, MintInfo>,
    ) -> Result<TransactionBuilder> {
        let to_close = self
            .get_account_state()
            .await?
            .positions()
            .filter(|p| p.balance == 0)
            .map(|p| {
                if p.adapter == glow_margin_pool::id() && p.kind() == TokenKind::Claim {
                    let pool = MarginPoolIxBuilder::new(
                        self.airspace(),
                        *loan_to_token.get(&p.token).unwrap(),
                    );
                    self.adapter_invoke_ix(pool.close_loan(*self.address(), self.ix.payer()), None)
                } else {
                    // Glow margin pools use token-2022, so any pool position should default to that token program
                    if p.adapter == glow_margin_pool::id() {
                        self.ix
                            .close_position(MintInfo::with_token_2022(p.token), p.address)
                    } else {
                        self.ix.close_position(
                            if p.is_token_2022 == 1 {
                                MintInfo::with_token_2022(p.token)
                            } else if p.is_token_2022 == 0 {
                                MintInfo::with_legacy(p.token)
                            } else {
                                panic!("Invalid token_2022 value")
                            },
                            p.address,
                        )
                    }
                }
            })
            .collect::<Vec<_>>();

        Ok(self.create_transaction_builder(&to_close))
    }

    /// Deposit tokens into a lending pool position owned by a margin account in
    /// an ATA position.
    ///
    /// Figures out if needed, and uses if so:
    /// - adapter vs accounting invoke
    /// - create position
    /// - refresh position
    ///
    /// # Params
    ///
    /// `underlying_mint` - The address of the mint for the tokens being deposited
    /// `source` - The token account that the deposit will be transferred from,
    ///            defaults to ata of source authority.
    /// `change` - The amount of tokens to deposit
    /// `authority` - The owner of the source account
    pub async fn pool_deposit(
        &self,
        underlying_mint: MintInfo,
        source: Option<Pubkey>,
        change: TokenChange,
        authority: MarginActionAuthority,
    ) -> Result<TransactionBuilder> {
        let target = self.pool_deposit_target(underlying_mint).await?;
        let source_authority = Some(authority.resolve(&self.ix));
        let tx = self
            .invoke_ctx()
            .pool_deposit(underlying_mint, source, source_authority, target, change)
            .ijoin();
        Ok(self.sign(tx))
    }

    async fn pool_deposit_target(&self, underlying_mint: MintInfo) -> Result<PoolTargetPosition> {
        let state = self.get_account_state().await?;
        // Moving this out of the async function will make eager calls
        let pool = self.get_pool(underlying_mint).await?;
        PoolTargetPosition::new(
            &state,
            &MarginPoolIxBuilder::new(self.airspace(), underlying_mint).deposit_note_mint,
            &self.rpc.payer().pubkey(),
            derive_pyth_price_feed_account(
                pool.token_price_oracle.pyth_feed_id().unwrap(),
                None,
                self.network_kind.pyth_oracle(),
            ),
            pool.token_price_oracle
                .pyth_redemption_feed_id()
                .map(|feed_id| {
                    derive_pyth_price_feed_account(feed_id, None, self.network_kind.pyth_oracle())
                }),
        )
        .await
    }

    /// DEPRECATED: use pool_deposit instead
    ///
    /// this uses the old style of registering positions (non-ata) which will
    /// stop being supported.
    ///
    /// TODO: can we remove this now?
    pub async fn pool_deposit_deprecated(
        &self,
        token_mint: MintInfo,
        source: Option<Pubkey>,
        change: TokenChange,
        authority: MarginActionAuthority,
    ) -> Result<TransactionBuilder> {
        let mut instructions = vec![];
        let authority = authority.resolve(&self.ix);
        let source = source.or_ata(&authority, &token_mint.address, &token_mint.token_program());
        let pool = MarginPoolIxBuilder::new(self.airspace(), token_mint);
        let (position, maybe_create) = self
            .get_or_create_position(pool.pool_deposit_mint_info())
            .await?;
        let inner_ix = pool.deposit(authority, Some(*self.address()), source, position, change);
        if let Some(create) = maybe_create {
            instructions.push(create);
            if self.ix.needs_signature(&inner_ix) {
                instructions.push(self.refresh_pool_position(token_mint).await?);
            }
        }
        instructions.push(self.smart_invoke(inner_ix, None));

        Ok(self.create_transaction_builder(&instructions))
    }

    /// Transaction to borrow tokens in a margin account
    ///
    /// # Params
    ///
    /// `token_mint` - The address of the mint for the tokens to borrow
    /// `amount` - The amount of tokens to borrow
    pub async fn borrow(
        &self,
        token_mint: MintInfo,
        change: TokenChange,
    ) -> Result<TransactionBuilder> {
        let mut instructions = vec![];
        let pool = MarginPoolIxBuilder::new(self.airspace(), token_mint);
        let token_metadata =
            get_token_metadata(&self.rpc, &self.airspace(), &token_mint.address).await?;
        let oracle = derive_pyth_price_feed_account(
            token_metadata.token_price_oracle.pyth_feed_id().unwrap(),
            None,
            self.network_kind.pyth_oracle(),
        );
        let redemption_price_oracle = token_metadata
            .token_price_oracle
            .pyth_redemption_feed_id()
            .map(|feed_id| {
                derive_pyth_price_feed_account(feed_id, None, self.network_kind.pyth_oracle())
            });

        self.with_liquidation_fee_accounts(token_mint, &mut instructions);

        let deposit_position = self
            .get_or_push_create_position(&mut instructions, pool.pool_deposit_mint_info())
            .await?;
        let _ = self
            .get_or_create_pool_loan_position(&mut instructions, &pool)
            .await?;

        let inner_refresh_loan_ix =
            pool.margin_refresh_position(self.ix.address, oracle, redemption_price_oracle);
        instructions.push(self.ix.accounting_invoke(inner_refresh_loan_ix));

        let inner_borrow_ix = pool.margin_borrow(self.ix.address, deposit_position, change);

        instructions.push(self.adapter_invoke_ix(inner_borrow_ix, Some(token_mint)));
        Ok(self.create_transaction_builder(&instructions))
    }

    /// Transaction to repay a loan of tokens in a margin account from the account's deposits
    ///
    /// # Params
    ///
    /// `token_mint` - The address of the mint for the tokens that were borrowed
    /// `amount` - The amount of tokens to repay
    pub async fn margin_repay(
        &self,
        token_mint: MintInfo,
        change: TokenChange,
    ) -> Result<TransactionBuilder> {
        let mut instructions = vec![];
        let pool = MarginPoolIxBuilder::new(self.airspace(), token_mint);

        self.with_liquidation_fee_accounts(token_mint, &mut instructions);

        let deposit_position = self
            .get_or_push_create_position(&mut instructions, pool.pool_deposit_mint_info())
            .await?;
        let _ = self
            .get_or_create_pool_loan_position(&mut instructions, &pool)
            .await?;

        let inner_repay_ix = pool.margin_repay(self.ix.address, deposit_position, change);

        instructions.push(self.adapter_invoke_ix(inner_repay_ix, Some(token_mint)));
        Ok(self.create_transaction_builder(&instructions))
    }

    /// Repay a loan from a token account of the underlying
    ///
    /// # Params
    ///
    /// `token_mint` - The address of the mint for the tokens that were borrowed
    /// `source` - Token account where funds originate, defaults to authority's ATA
    /// `change` - The amount of tokens to repay
    /// `authority` - The margin account who owns the loan and the tokens to repay
    pub fn pool_repay(
        &self,
        token_mint: MintInfo,
        source: Option<Pubkey>,
        change: TokenChange,
        authority: MarginActionAuthority,
    ) -> TransactionBuilder {
        let authority = authority.resolve(&self.ix);
        let source = source.or_ata(&authority, &token_mint.address, &token_mint.token_program());
        let pool = MarginPoolIxBuilder::new(self.airspace(), token_mint);
        let loan_notes = derive_loan_account(&self.ix.address, &pool.loan_note_mint);
        let inner_ix = pool.repay(authority, source, loan_notes, change);
        let wrapped_ix = self.smart_invoke(inner_ix, None);

        self.create_transaction_builder(&[wrapped_ix])
    }

    /// Transaction to withdraw tokens deposited into a margin account
    ///
    /// # Params
    ///
    /// `token_mint` - The address of the mint for the tokens to be withdrawn
    /// `amount` - The amount of tokens to withdraw
    pub async fn withdraw(
        &self,
        token_mint: MintInfo,
        destination: &Pubkey,
        change: TokenChange,
    ) -> Result<VersionedTransaction> {
        let mut instructions = vec![];
        let pool = MarginPoolIxBuilder::new(self.airspace(), token_mint);

        self.with_liquidation_fee_accounts(token_mint, &mut instructions);

        // TODO: a liquidator should not be able to withdraw to an account not owned by the margin account

        // Ensure the margin account's ATA exists; required by Withdraw context
        let create_margin_ata_ix = pool
            .token_mint
            .create_associated_token_account_idempotent(&self.ix.address, &self.ix.authority());
        instructions.push(create_margin_ata_ix);

        let deposit_position = self
            .get_or_push_create_position(&mut instructions, pool.pool_deposit_mint_info())
            .await?;

        let inner_withdraw_ix =
            pool.withdraw(self.ix.address, deposit_position, *destination, change);

        instructions.push(self.adapter_invoke_ix(inner_withdraw_ix, Some(token_mint)));
        self.create_transaction(&instructions).await
    }

    /// Swap from one pool on margin and deposit the output tokens to another pool
    /// The swap involves:
    /// - borrowing from pool to token account
    /// - swapping
    /// - repaying pool from token account
    /// - returning any remaining from source token to pool
    pub async fn margin_swap(
        &self,
        from_mint: MintInfo,
        to_mint: MintInfo,
        amount_out: u64,
        swap_instruction: &Instruction,
    ) -> Result<TransactionBuilder> {
        let mut instructions = vec![];
        let src_pool = MarginPoolIxBuilder::new(self.airspace(), from_mint);
        let dst_pool = MarginPoolIxBuilder::new(self.airspace(), to_mint);

        let borrow_dst = from_mint.associated_token_address(&self.ix.address);
        let deposit_src = to_mint.associated_token_address(&self.ix.address);
        // TODO: register these positions as margin token acocunts if they are supported, we can check this with TokenConfig
        let borrow_dst_ix = from_mint
            .create_associated_token_account_idempotent(&self.ix.address, &self.ix.authority());
        let deposit_src_ix = to_mint
            .create_associated_token_account_idempotent(&self.ix.address, &self.ix.authority());
        instructions.extend_from_slice(&[borrow_dst_ix, deposit_src_ix]);
        // Refresh the loan oracle
        let token_metadata =
            get_token_metadata(&self.rpc, &self.airspace(), &from_mint.address).await?;
        let oracle = derive_pyth_price_feed_account(
            token_metadata.token_price_oracle.pyth_feed_id().unwrap(),
            None,
            self.network_kind.pyth_oracle(),
        );
        let redemption_price_oracle = token_metadata
            .token_price_oracle
            .pyth_redemption_feed_id()
            .map(|feed_id| {
                derive_pyth_price_feed_account(feed_id, None, self.network_kind.pyth_oracle())
            });
        let inner_refresh_loan_ix =
            src_pool.margin_refresh_position(self.ix.address, oracle, redemption_price_oracle);
        let token_metadata =
            get_token_metadata(&self.rpc, &self.airspace(), &to_mint.address).await?;
        let oracle = derive_pyth_price_feed_account(
            token_metadata.token_price_oracle.pyth_feed_id().unwrap(),
            None,
            self.network_kind.pyth_oracle(),
        );
        let redemption_price_oracle = token_metadata
            .token_price_oracle
            .pyth_redemption_feed_id()
            .map(|feed_id| {
                derive_pyth_price_feed_account(feed_id, None, self.network_kind.pyth_oracle())
            });
        let inner_refresh_deposit_ix =
            dst_pool.margin_refresh_position(self.ix.address, oracle, redemption_price_oracle);

        // The balances before the swap have to be respected, and should not change after the swap.
        let borrow_dst_balance = self
            .rpc
            .get_token_balance(&borrow_dst)
            .await?
            .unwrap_or_default(); // We should not alter the balance after the swap
        let deposit_src_balance = self
            .rpc
            .get_token_balance(&deposit_src)
            .await?
            .unwrap_or_default();

        let loan_position = self
            .get_or_create_pool_loan_position(&mut instructions, &src_pool)
            .await?;
        // We use margin_borrow_v2 because we want the tokens borrowed to go to a token account for the swap
        let inner_borrow_ix = src_pool.margin_borrow_v2(self.ix.address, borrow_dst, amount_out);

        let deposit_position = self
            .get_or_push_create_position(&mut instructions, dst_pool.pool_deposit_mint_info())
            .await?;
        // The pool deposit after the swap has to take as much as it needs to leave the source balance with
        // a stipulated balance. This requires
        let inner_deposit_ix = dst_pool.deposit(
            self.ix.address,
            Some(*self.address()),
            deposit_src,
            deposit_position,
            TokenChange::set_source(deposit_src_balance),
        );
        let revert_bal_ix = src_pool.repay(
            self.ix.address,
            borrow_dst,
            loan_position,
            TokenChange::set_source(borrow_dst_balance),
        );
        let invoke_ix = self.adapter_invoke_many_ix(
            &[
                inner_refresh_loan_ix,
                inner_refresh_deposit_ix,
                inner_borrow_ix,
                swap_instruction.clone(),
                inner_deposit_ix,
                revert_bal_ix,
            ],
            None,
        );
        instructions.push(invoke_ix);

        Ok(self.create_transaction_builder(&instructions))
    }

    /// Swap from a deposit balance to repay a debt.
    ///
    /// The liquidation_fee parameter allows a liquidator to specify the fee they'll take
    /// if they're sure about it.
    pub async fn swap_and_repay(
        &self,
        from_mint: MintInfo,
        to_mint: MintInfo,
        swap_amount: u64,
        repay_amount: Option<u64>,
        swap_instruction: &Instruction,
        liquidation_fee: Option<u64>,
    ) -> Result<TransactionBuilder> {
        let mut instructions = vec![];
        let src_pool = MarginPoolIxBuilder::new(self.airspace(), from_mint);
        let dst_pool = MarginPoolIxBuilder::new(self.airspace(), to_mint);

        if self.is_liquidator {
            // Add liquidator fee account if it doesn't exist
            instructions.push(to_mint.create_associated_token_account_idempotent(
                &self.ix.authority(),
                &self.ix.authority(),
            ))
        }

        let liquidation_fee = if self.is_liquidator {
            liquidation_fee.unwrap_or_default()
        } else {
            0
        };

        let refresh_ixs = self.refresh_positions(self.address()).await?;
        // Combine them into a single transaction builder
        let mut builder = vec![];
        for (tx_b, _) in refresh_ixs {
            builder.push(tx_b);
        }
        let builder = builder.ijoin();

        // Get the account state and add other positions that need refreshing
        let state = self.get_account_state().await?;

        // Register positions for the ATA accounts so they're counted as part of equity
        instructions.push(self.ix.create_deposit_position(from_mint));
        instructions.push(self.ix.create_deposit_position(to_mint));

        // Important to refresh these positions as the program otherwise won't have their values
        let from_mint_config =
            get_position_config(&self.rpc, &self.airspace(), &from_mint.address).await?;
        let to_mint_config =
            get_position_config(&self.rpc, &self.airspace(), &to_mint.address).await?;
        // If there's no config for a token, the user might be swapping to an unsupported token.
        // TODO: DRY
        if let Some((_, token_config)) = from_mint_config {
            let TokenAdmin::Margin { oracle } = token_config.admin else {
                bail!("Invalid token oracle for from_mint");
            };
            let price_oracle = derive_pyth_price_feed_account(
                oracle.pyth_feed_id().unwrap(),
                None,
                self.network_kind.pyth_oracle(),
            );
            let redemption_price_oracle = oracle.pyth_redemption_feed_id().map(|feed_id| {
                derive_pyth_price_feed_account(feed_id, None, self.network_kind.pyth_oracle())
            });
            instructions.push(self.ix.refresh_deposit_position(
                from_mint,
                &price_oracle,
                redemption_price_oracle,
                true,
            ));
        }
        if let Some((_, token_config)) = to_mint_config {
            let TokenAdmin::Margin { oracle } = token_config.admin else {
                bail!("Invalid token oracle for from_mint");
            };
            let price_oracle = derive_pyth_price_feed_account(
                oracle.pyth_feed_id().unwrap(),
                None,
                self.network_kind.pyth_oracle(),
            );
            let redemption_price_oracle = oracle.pyth_redemption_feed_id().map(|feed_id| {
                derive_pyth_price_feed_account(feed_id, None, self.network_kind.pyth_oracle())
            });
            instructions.push(self.ix.refresh_deposit_position(
                to_mint,
                &price_oracle,
                redemption_price_oracle,
                true,
            ));
        }
        // From and To ATAs are the deposit positions created above
        let from_ata = from_mint.associated_token_address(&self.ix.address);
        let to_ata = to_mint.associated_token_address(&self.ix.address);
        // Refresh the relevant oracles
        let token_metadata =
            get_token_metadata(&self.rpc, &state.airspace, &from_mint.address).await?;
        let oracle = derive_pyth_price_feed_account(
            token_metadata.token_price_oracle.pyth_feed_id().unwrap(),
            None,
            self.network_kind.pyth_oracle(),
        );
        let redemption_price_oracle = token_metadata
            .token_price_oracle
            .pyth_redemption_feed_id()
            .map(|feed_id| {
                derive_pyth_price_feed_account(feed_id, None, self.network_kind.pyth_oracle())
            });
        let inner_refresh_src_pool_ix =
            src_pool.margin_refresh_position(self.ix.address, oracle, redemption_price_oracle);
        let token_metadata =
            get_token_metadata(&self.rpc, &state.airspace, &to_mint.address).await?;
        let oracle = derive_pyth_price_feed_account(
            token_metadata.token_price_oracle.pyth_feed_id().unwrap(),
            None,
            self.network_kind.pyth_oracle(),
        );
        let redemption_price_oracle = token_metadata
            .token_price_oracle
            .pyth_redemption_feed_id()
            .map(|feed_id| {
                derive_pyth_price_feed_account(feed_id, None, self.network_kind.pyth_oracle())
            });

        let inner_refresh_dst_pool_ix =
            dst_pool.margin_refresh_position(self.ix.address, oracle, redemption_price_oracle);

        // Track the ATA balance to preserve it throughout the swap
        let to_ata_balance = self
            .rpc
            .get_token_balance(&to_ata)
            .await?
            .unwrap_or_default();

        let src_pool_deposit_position = self
            .get_or_push_create_position(&mut instructions, src_pool.pool_deposit_mint_info())
            .await?;
        let inner_src_pool_withdraw_ix = src_pool.withdraw(
            self.ix.address,
            src_pool_deposit_position,
            from_ata,
            TokenChange::shift(swap_amount),
        );

        let dst_pool_loan_position = self
            .get_or_create_pool_loan_position(&mut instructions, &dst_pool)
            .await?;
        let inner_dst_pool_repay_ix = dst_pool.repay(
            self.ix.address,
            to_ata,
            dst_pool_loan_position,
            match repay_amount {
                Some(amount) => TokenChange::shift(amount - liquidation_fee),
                None => TokenChange::set_source(to_ata_balance + liquidation_fee),
            },
        );
        // Dump remaining tokens in a deposit account
        let dst_pool_deposit_position = self
            .get_or_push_create_position(&mut instructions, dst_pool.pool_deposit_mint_info())
            .await?;
        let inner_dst_pool_deposit_ix = dst_pool.deposit(
            self.ix.address,
            Some(self.ix.address),
            to_ata,
            dst_pool_deposit_position,
            TokenChange::set_source(to_ata_balance + liquidation_fee),
        );
        let invoke_ix = self.adapter_invoke_many_ix(
            &[
                inner_refresh_src_pool_ix,
                inner_refresh_dst_pool_ix,
                inner_src_pool_withdraw_ix,
                swap_instruction.clone(),
                inner_dst_pool_repay_ix,
                inner_dst_pool_deposit_ix,
            ],
            Some(to_mint),
        );
        instructions.push(invoke_ix);

        let swap_builder = self.create_transaction_builder(&instructions);

        Ok([builder, swap_builder].ijoin())
    }

    /// Set up any accounts required for taking liquidation fees if the user is a liquidator
    fn with_liquidation_fee_accounts(
        &self,
        fee_mint: MintInfo,
        instructions: &mut Vec<Instruction>,
    ) {
        if self.is_liquidator {
            // Create the ATA of the liquidator and margin account as either might not exist
            instructions.push(fee_mint.create_associated_token_account_idempotent(
                &self.margin_account_address(),
                &self.ix.authority(),
            ));
            instructions.push(fee_mint.create_associated_token_account_idempotent(
                &self.ix.authority(),
                &self.ix.authority(),
            ));
        }
    }

    fn sign<Tx: WithSigner>(&self, tx: Tx) -> Tx::Output {
        match self.signer.as_ref() {
            Some(signer) => tx.with_signer(signer.clone()),
            None => tx.without_signer(),
        }
    }

    /// Transaction to begin liquidating user account.
    /// If `refresh_position` is provided, all the margin pools will be refreshed first.
    pub async fn liquidate_begin(&self, refresh_positions: bool) -> Result<VersionedTransaction> {
        let builder = self.liquidate_begin_builder(refresh_positions).await?;
        self.rpc.compile(builder).await
    }

    /// Transaction to begin liquidating user account.
    /// If `refresh_position` is provided, all the margin pools will be refreshed first.
    pub async fn liquidate_begin_builder(
        &self,
        refresh_positions: bool,
    ) -> Result<TransactionBuilder> {
        assert!(self.is_liquidator);

        // Get the margin account and refresh positions
        let mut txs = if refresh_positions {
            cat![
                self.refresh_all_pool_positions()
                    .await?
                    .into_iter()
                    .map(|v| v.0)
                    .collect::<Vec<_>>(),
                self.refresh_deposit_positions()
                    .await?
                    .into_iter()
                    .map(|v| v.0)
                    .collect::<Vec<_>>(),
            ]
            .ijoin()
        } else {
            TransactionBuilder::default()
        };

        // Add liquidation instruction
        txs.instructions.push(self.ix.liquidate_begin());
        txs.signers
            .push(self.signer.as_ref().context("missing signer")?.clone());

        Ok(txs)
    }

    /// Collect the liquidation fee in specified token
    pub async fn collect_liquidation_fees(&self) -> Result<Vec<TransactionBuilder>> {
        let pyth_oracle = self.network_kind.pyth_oracle();
        assert!(self.is_liquidator);
        let liquidator = self.ix.authority();
        let margin_account = self.address();

        // Get liquidation state and list the tokens that are eligible for a fee
        let liquidation_state = self
            .rpc()
            .get_account(&derive_liquidation(*margin_account, liquidator))
            .await?
            .expect("Liquidation account should exist");
        let liquidation_state =
            bytemuck::pod_read_unaligned::<LiquidationState>(&liquidation_state.data[8..]);
        let margin_account_state = self.get_account_state().await?;

        let liquidation_fees = liquidation_state
            .state
            .accrued_liquidation_fees
            .iter()
            .filter(|f| f.mint != Pubkey::default());
        // To collect fees, the liquidator should withdraw the desired amounts from pools to the user's margin tokens
        let mut transfer_ixs = vec![];
        let mut collect_ixs = vec![];
        for fee in liquidation_fees {
            let mint = self.rpc().get_account(&fee.mint).await?.unwrap();
            let mint = MintInfo::with_token_program(fee.mint, mint.owner);
            // Check if the user has a position with sufficient balance, else transfer
            // TODO: we can subtract equity loss to minimize transfer amount
            let pool = MarginPoolIxBuilder::new(self.airspace(), mint);
            let deposit_position = self
                .get_or_push_create_position(&mut transfer_ixs, pool.pool_deposit_mint_info())
                .await?;
            transfer_ixs.push(
                mint.create_associated_token_account_idempotent(margin_account, &self.ix.payer()),
            );
            let existing_position = margin_account_state
                .positions()
                .find(|p| p.token == mint.address);
            if let Some(position) = existing_position {
                if position.balance < fee.amount {
                    // Withdraw from margin pool
                    transfer_ixs.push(self.ix.liquidator_invoke(
                        pool.withdraw(
                            *margin_account,
                            deposit_position,
                            mint.associated_token_address(margin_account),
                            TokenChange::set_destination(fee.amount),
                        ),
                        mint,
                    ));
                }
            }
            // Create ATAs
            collect_ixs.push(
                mint.create_associated_token_account_idempotent(&liquidator, &self.ix.payer()),
            );
            // Get the token config
            let token_config = derive_token_config(&self.ix.airspace_details.address, &fee.mint);
            let token_config_data = self.rpc().get_account(&token_config).await?.unwrap();
            let token_config_data = TokenConfig::try_deserialize(&mut &token_config_data.data[..])?;
            let TokenAdmin::Margin { oracle } = token_config_data.admin else {
                bail!("Expected token admin to be an oracle, cannot collect fees");
            };
            let feed_id = oracle.pyth_feed_id().unwrap();
            let quote_feed_id = oracle.pyth_redemption_feed_id();
            let price_oracle = derive_pyth_price_feed_account(feed_id, None, pyth_oracle);
            let redemption_quote_oracle = quote_feed_id
                .map(|feed_id| derive_pyth_price_feed_account(feed_id, None, pyth_oracle));
            collect_ixs.push(self.ix.collect_liquidation_fee(
                mint,
                token_config,
                price_oracle,
                redemption_quote_oracle,
            ))
        }

        // Add signer
        let mut transfer_builder: TransactionBuilder = transfer_ixs.into();
        transfer_builder
            .signers
            .push(self.signer.as_ref().context("missing signer")?.clone());
        let mut collect_builder: TransactionBuilder = collect_ixs.into();
        collect_builder
            .signers
            .push(self.signer.as_ref().context("missing signer")?.clone());

        Ok(vec![transfer_builder, collect_builder])
    }

    /// Transaction to end liquidating user account
    pub async fn liquidate_end(
        &self,
        original_liquidator: Option<Pubkey>,
    ) -> Result<VersionedTransaction> {
        self.create_transaction(&[self.ix.liquidate_end(original_liquidator)])
            .await
    }

    /// Verify that the margin account is healthy
    pub async fn verify_healthy(&self) -> Result<VersionedTransaction> {
        self.create_unsigned_transaction(&[self.ix.verify_healthy()])
            .await
    }

    /// Verify that the margin account is unhealthy
    pub async fn verify_unhealthy(&self) -> Result<VersionedTransaction> {
        self.create_unsigned_transaction(&[self.ix.verify_unhealthy()])
            .await
    }

    /// Refresh a user's position in a margin pool
    pub async fn refresh_pool_position(&self, token_mint: MintInfo) -> Result<Instruction> {
        let ix_builder = MarginPoolIxBuilder::new(self.airspace(), token_mint);
        let pool_oracle = self.get_pool(token_mint).await?.token_price_oracle;
        let oracle = derive_pyth_price_feed_account(
            pool_oracle.pyth_feed_id().unwrap(),
            None,
            self.network_kind.pyth_oracle(),
        );
        let redemption_price_oracle = pool_oracle.pyth_redemption_feed_id().map(|feed_id| {
            derive_pyth_price_feed_account(feed_id, None, self.network_kind.pyth_oracle())
        });

        Ok(self
            .ix
            .accounting_invoke(ix_builder.margin_refresh_position(
                self.ix.address,
                oracle,
                redemption_price_oracle,
            )))
    }

    /// Append instructions to refresh pool positions to instructions
    pub async fn refresh_all_pool_positions(
        &self,
    ) -> Result<Vec<(TransactionBuilder, TokenPriceOracle)>> {
        Ok(self
            .refresh_all_pool_positions_underlying_to_tx()
            .await?
            .into_values()
            .collect())
    }

    /// Refresh metadata for all positions in the user account
    pub async fn refresh_all_position_metadata(&self) -> Result<Vec<TransactionBuilder>> {
        let instructions = self
            .get_account_state()
            .await?
            .positions()
            .map(|position| {
                self.ix
                    .refresh_position_config(&position.token)
                    .with_signers(self.signers())
            })
            .collect::<Vec<_>>();

        Ok(instructions)
    }

    /// Create a new token account that accepts deposits, registered as a position
    pub async fn create_deposit_position(
        &self,
        token_mint: MintInfo,
    ) -> Result<VersionedTransaction> {
        self.create_transaction(&[
            token_mint.create_associated_token_account_idempotent(self.address(), &self.signer()),
            self.ix.create_deposit_position(token_mint),
        ])
        .await
    }

    /// Close a previously created deposit account
    pub async fn close_deposit_position(
        &self,
        token_mint: MintInfo,
    ) -> Result<VersionedTransaction> {
        let token_account = token_mint.associated_token_address(self.address());
        let instruction = self.ix.close_position(token_mint, token_account);
        self.create_transaction(&[instruction]).await
    }

    /// Transfer tokens into or out of a deposit account associated with the margin account
    pub async fn transfer_deposit(
        &self,
        token_mint: MintInfo,
        source_owner: Pubkey,
        source: Pubkey,
        destination: Pubkey,
        amount: u64,
    ) -> Result<TransactionBuilder> {
        let state = self.get_account_state().await?;
        let mut instructions = vec![];

        if !state.positions().any(|p| p.token == token_mint.address) {
            instructions.push(
                token_mint
                    .create_associated_token_account_idempotent(self.address(), &self.signer()),
            );
            instructions.push(self.ix.create_deposit_position(token_mint));
        }

        instructions.push(self.ix.transfer_deposit(
            source_owner,
            source,
            destination,
            token_mint,
            amount,
        ));

        Ok(self.create_transaction_builder(&instructions))
    }

    /// Get the latest [MarginAccount] state
    pub async fn get_account_state(&self) -> Result<Box<MarginAccount>> {
        Ok(Box::new(
            get_margin_account(&self.rpc, &self.ix.address).await?,
        ))
    }

    /// Append instructions to refresh pool positions to instructions
    pub async fn refresh_all_pool_positions_underlying_to_tx(
        &self,
    ) -> Result<HashMap<Pubkey, (TransactionBuilder, TokenPriceOracle)>> {
        let state = self.get_account_state().await?;
        refresh_all_pool_positions_underlying_to_tx(&self.rpc, &state).await
    }

    /// Append instructions to refresh deposit positions
    pub async fn refresh_deposit_positions(
        &self,
    ) -> Result<Vec<(TransactionBuilder, TokenPriceOracle)>> {
        let state = self.get_account_state().await?;
        refresh_deposit_positions(&self.rpc, &state).await
    }

    async fn get_pool(&self, token_mint: MintInfo) -> Result<MarginPool> {
        let pool_builder = MarginPoolIxBuilder::new(self.airspace(), token_mint);
        let account = self
            .rpc
            .get_account(&pool_builder.address)
            .await?
            .context("could not find pool")?;

        Ok(MarginPool::try_deserialize(&mut &account.data[..])?)
    }

    async fn get_or_push_create_position(
        &self,
        instructions: &mut Vec<Instruction>,
        token_mint: MintInfo,
    ) -> Result<Pubkey> {
        let (address, create) = self.get_or_create_position(token_mint).await?;
        if let Some(ix) = create {
            instructions.push(ix);
        }
        Ok(address)
    }

    async fn get_or_create_position(
        &self,
        token_mint: MintInfo,
    ) -> Result<(Pubkey, Option<Instruction>)> {
        match self.get_position_token_account(&token_mint.address).await? {
            Some(address) => Ok((address, None)),
            None => Ok((
                derive_position_token_account(&self.ix.address, &token_mint.address),
                Some(self.ix.register_position(token_mint)),
            )),
        }
    }

    async fn get_position_token_account(&self, token_mint: &Pubkey) -> Result<Option<Pubkey>> {
        Ok(self
            .get_account_state()
            .await?
            .positions()
            .find(|p| p.token == *token_mint)
            .map(|p| p.address))
    }

    async fn get_or_create_pool_loan_position(
        &self,
        instructions: &mut Vec<Instruction>,
        pool: &MarginPoolIxBuilder,
    ) -> Result<Pubkey> {
        let state = self.get_account_state().await?;
        let search_result = state.positions().find(|p| p.token == pool.loan_note_mint);

        Ok(if let Some(position) = search_result {
            position.address
        } else {
            let pools_ix = pool.register_loan(self.ix.address, self.ix.payer());
            let wrapped_ix = self.adapter_invoke_ix(pools_ix, None);
            instructions.push(wrapped_ix);

            derive_loan_account(&self.ix.address, &pool.loan_note_mint)
        })
    }

    fn adapter_invoke_ix(&self, inner: Instruction, fee_mint: Option<MintInfo>) -> Instruction {
        match self.is_liquidator {
            true => self.ix.liquidator_invoke(inner, fee_mint.unwrap()),
            false => self.ix.adapter_invoke(inner),
        }
    }

    fn adapter_invoke_many_ix(
        &self,
        inners: &[Instruction],
        fee_mint: Option<MintInfo>,
    ) -> Instruction {
        match self.is_liquidator {
            true => self.ix.liquidator_invoke_many(inners, fee_mint.unwrap()),
            false => self.ix.adapter_invoke_many(inners),
        }
    }

    /// If the margin account needs to sign, then use adapter or liquidator
    /// invoke, otherwise use accounting invoke.
    pub fn smart_invoke(&self, inner: Instruction, fee_mint: Option<MintInfo>) -> Instruction {
        if self.ix.needs_signature(&inner) {
            self.adapter_invoke_ix(inner, fee_mint)
        } else {
            self.ix.accounting_invoke(inner)
        }
    }
}

/// Instructions invoked through a margin account may require a signer that
/// could potentially be any account, depending on the situation. For example, a
/// deposit into the margin account requires a signer from the source account,
/// which could be anyone.
///
/// Most cases follow one of a few common patterns though. For example the
/// margin account authority or the margin account itself is most likely to be
/// the account authorizing a deposit. But in theory it could be anyone.
///
/// Rather than requiring the caller to always specify the address of the
/// authority of this action, we can leverage some of the data that is already
/// encapsulated within the MarginIxBuilder. So the caller of the function can
/// just specify that it wants to use a concept, such as "authority", rather
/// than having to struggle to identify the authority.
pub enum MarginActionAuthority {
    /// - The builder's configured "authority" for the margin account.
    /// - Typically, the account owner or its liquidator, depending on context.
    /// - See method: `MarginIxBuilder::authority()`.
    /// - In theory, this is *expected* to be whatever the actual MarginAccount
    ///   on chain is configured to require as the authority for user actions,
    ///   but there is nothing in MarginIxBuilder that guarantees its
    ///   "authority" is consistent with the on-chain state.
    AccountAuthority,
    /// The margin account itself is the authority, so there is no external
    /// signature needed.
    MarginAccount,
    /// Some other account that the tx_builder doesn't know about needs to sign.
    AdHoc(Pubkey),
}

impl MarginActionAuthority {
    fn resolve(self, ixb: &MarginIxBuilder) -> Pubkey {
        match self {
            MarginActionAuthority::AccountAuthority => ixb.authority(),
            MarginActionAuthority::MarginAccount => ixb.address,
            MarginActionAuthority::AdHoc(adhoc) => adhoc,
        }
    }
}
