use std::{collections::HashSet, convert::identity, sync::Arc};

use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use glow_program_common::token_change::TokenChange;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};

use glow_instructions::{
    derive_pyth_price_feed_account,
    margin_pool::{derive_loan_account, MarginPoolIxBuilder},
    MintInfo,
};

use crate::{
    client::{ClientResult, ClientState},
    margin::MarginAccountClient,
};

/// Client for interacting with a margin pool, from the perspective of a margin account
#[derive(Clone)]
pub struct MarginAccountPoolClient {
    pub(crate) client: Arc<ClientState>,
    pub(crate) builder: MarginPoolIxBuilder,
    pub(crate) account: MarginAccountClient,
}

impl MarginAccountPoolClient {
    pub fn new(account: MarginAccountClient, token: MintInfo) -> Self {
        let builder = MarginPoolIxBuilder::new(account.airspace(), token);

        Self {
            client: account.client.clone(),
            builder,
            account,
        }
    }

    /// Lend tokens from the margin account into the pool
    ///
    /// This will transfer tokens currently in the user's margin account as a deposit into
    /// this margin pool.
    ///
    /// # Parameters
    ///
    /// * `amount` - The token amount to transfer for lending
    pub async fn lend(&self, amount: u64) -> ClientResult<()> {
        let mut ixns = vec![];
        let lending_deposit_account = self
            .account
            .with_deposit_position(self.builder.pool_deposit_mint_info(), &mut ixns)
            .await?;

        let token_account = self
            .builder
            .token_mint
            .associated_token_address(&self.account.address);

        ixns.push(self.account.builder.adapter_invoke(self.builder.deposit(
            self.account.address,
            Some(self.account.address),
            token_account,
            lending_deposit_account,
            TokenChange::shift(amount),
        )));

        self.client.send(&ixns).await
    }

    /// Deposit tokens from wallet directly into the pool for lending in the margin account.
    ///
    /// # Parameters
    ///
    /// * `amount` - The token amount to transfer for lending
    /// * `source` - The source token account to deposit from. If `None`, then assumes the
    ///              source as the associated token account for the current user wallet.
    pub async fn deposit(&self, amount: u64, source: Option<&Pubkey>) -> ClientResult<()> {
        let mut ixns = vec![];
        let lending_deposit_account = self
            .account
            .with_deposit_position(self.builder.pool_deposit_mint_info(), &mut ixns)
            .await?;

        let token_account = source.cloned().unwrap_or_else(|| {
            self.builder
                .token_mint
                .associated_token_address(&self.client.signer())
        });

        ixns.push(self.builder.deposit(
            self.client.signer(),
            Some(self.account.address),
            token_account,
            lending_deposit_account,
            TokenChange::shift(amount),
        ));

        ixns.push(
            self.account
                .builder
                .update_position_balance(lending_deposit_account),
        );

        self.client.send(&ixns).await
    }

    /// Withdraw tokens from the pool, directly to the user's wallet
    ///
    /// # Parameters
    ///
    /// * `amount` - The token amount to be withdrawn. If `None`, the maximum amount is withdrawn.
    /// * `destination` - The token account to receive the withdrawn tokens. If `None`, then assumes
    ///                   the destination is the associated token account for the current user wallet
    pub async fn withdraw(
        &self,
        amount: Option<u64>,
        destination: Option<&Pubkey>,
    ) -> ClientResult<()> {
        let deposit_account = get_associated_token_address_with_program_id(
            &self.account.address,
            &self.builder.deposit_note_mint,
            &self.builder.pool_deposit_mint_info().token_program(),
        );

        let mut ixns = vec![];

        // Ensure the depositor's ATA (margin account ATA) exists; required by Withdraw context
        let create_margin_ata = self
            .builder
            .token_mint
            .create_associated_token_account_idempotent(
                &self.account.address,
                &self.client.signer(),
            );
        ixns.push(create_margin_ata);

        let deposit_destination = match destination {
            Some(dest) => *dest,
            None => {
                self.client
                    .with_wallet_account(self.builder.token_mint, &mut ixns)
                    .await?
            }
        };

        let change = match amount {
            None => TokenChange::set_source(0),
            Some(value) => TokenChange::shift(value),
        };

        ixns.push(self.account.builder.adapter_invoke(self.builder.withdraw(
            self.account.address,
            deposit_account,
            deposit_destination,
            change,
        )));

        self.account.send_with_refresh(&ixns).await
    }

    /// Borrow tokens from the pool, withdrawing them to the user's wallet
    ///
    /// # Parameters
    ///
    /// * `amount` - The number of tokens to borrow
    /// * `destination` - The token account to place the borrowed tokens in. If `None`, defaults to
    ///                   the associated token account for the user.
    pub async fn borrow_withdraw(
        &self,
        amount: u64,
        destination: Option<&Pubkey>,
    ) -> ClientResult<()> {
        let mut ixns = vec![];

        let token_account = match destination {
            Some(address) => *address,
            None => {
                self.client
                    .with_wallet_account(self.builder.token_mint, &mut ixns)
                    .await?
            }
        };

        ixns.extend(self.with_create_loan_account().await?);
        ixns.push(
            self.account
                .builder
                .adapter_invoke(self.builder.margin_borrow_v2(
                    self.account.address,
                    token_account,
                    amount,
                )),
        );

        self.account.send_with_refresh(&ixns).await
    }

    /// Borrow tokens from the pool
    ///
    /// # Parameters
    ///
    /// * `amount` - The token amount to borrow
    /// * `destination` - The account to receive the borrowed tokens
    pub async fn borrow(&self, amount: u64, destination: Option<&Pubkey>) -> ClientResult<()> {
        let mut ixns = vec![];

        let destination = match destination {
            Some(address) => *address,
            None => {
                self.account
                    .with_deposit_position(self.builder.token_mint, &mut ixns)
                    .await?
            }
        };

        ixns.extend(self.with_create_loan_account().await?);
        ixns.push(
            self.account
                .builder
                .adapter_invoke(self.builder.margin_borrow_v2(
                    self.account.address,
                    destination,
                    amount,
                )),
        );

        self.account.send_with_refresh(&ixns).await
    }

    /// Cancel a loan by repaying from an existing deposit in the pool
    ///
    /// # Parameters
    ///
    /// * `amount` - The token amount to transfer as a repayment for a loan. If `None`, transfers
    ///              the maximum amount of tokens required to fully pay off the total loan.
    pub async fn cancel_borrow(&self, amount: Option<u64>) -> ClientResult<()> {
        let deposit_account = self
            .account
            .builder
            .get_token_account_address(&self.builder.deposit_note_mint);

        let change = match amount {
            None => TokenChange::set_destination(0),
            Some(value) => TokenChange::shift(value),
        };
        let mut instructions =
            vec![self
                .account
                .builder
                .adapter_invoke(self.builder.margin_repay(
                    self.account.address,
                    deposit_account,
                    change,
                ))];

        if amount.is_none() {
            instructions.push(
                self.account.builder.adapter_invoke(
                    self.builder
                        .close_loan(self.account.address, self.client.signer()),
                ),
            )
        }

        self.client.send(&instructions).await
    }

    /// Deposit tokens from user wallet in order to repay a loan
    ///
    /// # Parameters
    ///
    /// * `amount` - The token amount to transfer as a repayment for a loan. If `None`, transfers
    ///              the maximum amount of tokens required to fully pay off the total loan.
    /// * `source` - The source token account to deposit from. If `None`, then assumes the
    ///              source as the associated token account for the current user wallet.
    pub async fn deposit_repay(
        &self,
        amount: Option<u64>,
        source: Option<&Pubkey>,
    ) -> ClientResult<()> {
        let token_account = source.cloned().unwrap_or_else(|| {
            self.builder
                .token_mint
                .associated_token_address(&self.client.signer())
        });

        self.repay_from(&self.client.signer(), &token_account, amount, identity)
            .await
    }

    /// Use tokens from user margin account in order to repay a loan
    ///
    /// # Parameters
    ///
    /// * `amount` - The token amount to transfer as a repayment for a loan. If `None`, transfers
    ///              the maximum amount of tokens required to fully pay off the total loan.
    pub async fn repay(&self, amount: Option<u64>) -> ClientResult<()> {
        let token_account = self
            .builder
            .token_mint
            .associated_token_address(&self.account.address);

        self.repay_from(&self.account.address, &token_account, amount, |repay| {
            self.account.builder.adapter_invoke(repay)
        })
        .await
    }

    fn instruction_for_refresh(&self) -> ClientResult<Instruction> {
        let token_info = self
            .client
            .state()
            .token_info(&self.builder.token_mint.address)?;
        let oracle = derive_pyth_price_feed_account(
            &token_info.pyth_feed_id.unwrap(),
            None,
            self.client.network_kind.pyth_oracle(),
        );
        let redemption_price_oracle = token_info.pyth_redemption_feed_id.map(|feed_id| {
            derive_pyth_price_feed_account(&feed_id, None, self.client.network_kind.pyth_oracle())
        });

        Ok(self
            .account
            .builder
            .accounting_invoke(self.builder.margin_refresh_position(
                self.account.address,
                oracle,
                redemption_price_oracle,
            )))
    }

    async fn repay_from(
        &self,
        authority: &Pubkey,
        token_account: &Pubkey,
        amount: Option<u64>,
        proxy_ix: impl Fn(Instruction) -> Instruction,
    ) -> ClientResult<()> {
        let loan_account = derive_loan_account(&self.account.address, &self.builder.loan_note_mint);
        let change = match amount {
            None => TokenChange::set_destination(0),
            Some(value) => TokenChange::shift(value),
        };
        let mut instructions = vec![
            proxy_ix(
                self.builder
                    .repay(*authority, *token_account, loan_account, change),
            ),
            self.account.builder.update_position_balance(loan_account),
        ];

        if amount.is_none() {
            instructions.push(
                self.account.builder.adapter_invoke(
                    self.builder
                        .close_loan(self.account.address, self.client.signer()),
                ),
            )
        }

        self.client.send(&instructions).await
    }

    async fn with_create_loan_account(&self) -> ClientResult<Vec<Instruction>> {
        let loan_account = derive_loan_account(&self.account.address, &self.builder.loan_note_mint);

        match self.client.account_exists(&loan_account).await? {
            true => Ok(vec![]),
            false => Ok(vec![
                self.account.builder.adapter_invoke(
                    self.builder
                        .register_loan(self.account.address, self.client.signer()),
                ),
                self.instruction_for_refresh()?,
            ]),
        }
    }
}

pub(crate) fn instruction_for_refresh(
    account: &MarginAccountClient,
    token: MintInfo,
    refreshing_tokens: &mut HashSet<Pubkey>,
) -> ClientResult<Instruction> {
    let token_config = account.token_config(&token.address)?;
    let pool_client = MarginAccountPoolClient::new(
        account.clone(),
        MintInfo::with_token_program(
            token_config.underlying_mint,
            token_config.underlying_mint_token_program,
        ),
    );

    refreshing_tokens.insert(pool_client.builder.deposit_note_mint);
    refreshing_tokens.insert(pool_client.builder.loan_note_mint);

    pool_client.instruction_for_refresh()
}
