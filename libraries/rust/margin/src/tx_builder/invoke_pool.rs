use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anyhow::Result;
use glow_margin::MarginAccount;
use glow_program_common::token_change::TokenChange;
use glow_solana_client::transaction::TransactionBuilder;
use glow_solana_client::transactions;
use glow_solana_client::util::Key;
use solana_sdk::pubkey::Pubkey;

use crate::ix_builder::*;
use crate::solana::pubkey::OrAta;

use super::MarginInvokeContext;

/// Use MarginInvokeContext to invoke instructions to the margin-pool program
impl MarginInvokeContext {
    /// Deposit into a margin pool from the specified source account, creating
    /// the target position in the margin account if necessary.
    pub fn pool_deposit(
        &self,
        underlying_mint: MintInfo,
        source: Option<Pubkey>,
        source_authority: Option<Pubkey>,
        target: PoolTargetPosition,
        change: TokenChange,
    ) -> Vec<TransactionBuilder> {
        let pool = MarginPoolIxBuilder::new(self.airspace, underlying_mint);
        let source_authority = source_authority.unwrap_or(self.margin_account);
        let (target, mut instructions) = self.get_or_create_pool_deposit(underlying_mint, target);
        instructions.push(self.invoke(
            pool.deposit(
                source_authority,
                Some(self.margin_account),
                source.or_ata(
                    &source_authority,
                    &underlying_mint.address,
                    &underlying_mint.token_program(),
                ),
                target,
                change,
            ),
            if self.is_liquidator {
                Some(underlying_mint)
            } else {
                None
            },
        ));
        instructions
    }

    /// Return the address to a pool deposit for this user, including
    /// instructions to create and refresh the position if necessary.
    pub fn get_or_create_pool_deposit(
        &self,
        underlying_mint: MintInfo,
        position: PoolTargetPosition,
    ) -> (Pubkey, Vec<TransactionBuilder>) {
        let pool = MarginPoolIxBuilder::new(self.airspace, underlying_mint);
        let mut instructions = vec![];
        let target = match position {
            PoolTargetPosition::Existing(pos) => pos,
            PoolTargetPosition::NeedNew {
                payer,
                pool_oracle,
                pool_redemption_oracle,
            } => {
                instructions.extend(self.create_pool_deposit(
                    payer,
                    underlying_mint,
                    pool_oracle,
                    pool_redemption_oracle,
                ));
                get_associated_token_address_with_program_id(
                    &self.margin_account,
                    &pool.deposit_note_mint,
                    &pool.pool_deposit_mint_info().token_program(),
                )
            }
        };
        (target, instructions)
    }

    /// Create and refresh a pool deposit position
    pub fn create_pool_deposit(
        &self,
        payer: Pubkey,
        underlying_mint: MintInfo,
        pool_oracle: Pubkey,
        pool_redemption_rate_oracle: Option<Pubkey>,
    ) -> Vec<TransactionBuilder> {
        let pool = MarginPoolIxBuilder::new(self.airspace, underlying_mint);
        let auth = self.authority.address();

        transactions![
            create_deposit_account_and_position(
                self.margin_account,
                self.airspace,
                auth,
                payer,
                pool.pool_deposit_mint_info(),
            ),
            self.invoke(
                pool.margin_refresh_position(
                    self.margin_account,
                    pool_oracle,
                    pool_redemption_rate_oracle
                ),
                if self.is_liquidator {
                    Some(underlying_mint)
                } else {
                    None
                },
            ),
        ]
    }
}

/// An instruction needs to allocate a non-zero balance into a pool position.
/// This type represents whether or not the position exists:
/// - if so, it provides the address of the token account for that position.
/// - if not, it provides the data that will be necessary to create and refresh
///   the position, so it can successfully acquire a balance.
pub enum PoolTargetPosition {
    /// The position already exists at the provided token account
    Existing(Pubkey),
    /// The position does not exist. Use this data to create and refresh it.
    NeedNew {
        /// funds the creation of the token account
        payer: Pubkey,
        /// needed to refresh the position
        pool_oracle: Pubkey,
        /// redemption rate if the pool depends on one
        pool_redemption_oracle: Option<Pubkey>,
    },
}

impl PoolTargetPosition {
    /// common pattern to figure out what information is needed to target a pool
    /// position.
    pub async fn new<E>(
        margin_account: &MarginAccount,
        position_token_mint: &Pubkey,
        payer: &Pubkey,
        pool_oracle: Pubkey,
        pool_redemption_oracle: Option<Pubkey>,
    ) -> Result<PoolTargetPosition, E> {
        Ok(match margin_account.get_position(position_token_mint) {
            Some(pos) => PoolTargetPosition::Existing(pos.address),
            None => PoolTargetPosition::NeedNew {
                pool_oracle,
                pool_redemption_oracle,
                payer: *payer,
            },
        })
    }
}
