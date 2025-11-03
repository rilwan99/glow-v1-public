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

use std::cmp::Ordering;
use std::convert::TryFrom;

use anchor_lang::{prelude::*, solana_program::clock::UnixTimestamp};
use glow_margin::MAX_ORACLE_STALENESS;
use glow_program_common::oracle::TokenPriceOracle;
use glow_program_common::token_change::{ChangeKind, TokenChange};
use glow_program_common::{Number, BPS_EXPONENT};

use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
#[cfg(any(test, feature = "no-entrypoint"))]
use serde::{
    ser::{SerializeStruct, Serializer},
    Deserialize, Serialize,
};

use crate::{util, Amount, AmountKind, ErrorCode, MAX_POOL_UTIL_RATIO_AFTER_BORROW_BPS};

/// Account containing information about a margin pool, which
/// services lending/borrowing operations.
#[account]
#[repr(C, align(8))]
#[derive(Default)]
pub struct MarginPool {
    pub version: u8,

    /// The bump seed used to create the pool address
    pub pool_bump: [u8; 1],

    /// The address of pool's airspace
    pub airspace: Pubkey,

    /// The address of the vault account, which has custody of the
    /// pool's tokens
    pub vault: Pubkey,

    /// The address of the account to deposit collected fees, represented as
    /// deposit notes
    pub fee_destination: Pubkey,

    /// The address of the mint for deposit notes
    pub deposit_note_mint: Pubkey,

    /// The address of the mint for the loan notes
    pub loan_note_mint: Pubkey,

    /// The token the pool allows lending and borrowing on
    pub token_mint: Pubkey,

    /// The address of this pool
    pub address: Pubkey,

    /// The configuration of the pool
    pub config: MarginPoolConfig,

    /// The total amount of tokens borrowed, that need to be repaid to
    /// the pool.
    pub borrowed_tokens: [u8; 24],

    /// The total amount of tokens in the pool that's reserved for collection
    /// as fees.
    pub uncollected_fees: [u8; 24],

    /// The total amount of tokens available in the pool's vault
    pub deposit_tokens: u64,

    /// The total amount of notes issued to depositors of tokens.
    pub deposit_notes: u64,

    /// The total amount of notes issued to borrowers of tokens
    pub loan_notes: u64,

    /// The time the interest was last accrued up to
    pub accrued_until: i64,

    /// Details about the price oracle
    ///
    /// SECURITY: This value also exists in the token metadata, and should be updated in sync
    /// with it.
    pub token_price_oracle: TokenPriceOracle,
}

impl std::fmt::Debug for MarginPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarginPool")
            .field("version", &self.version)
            .field("pool_bump", &self.pool_bump)
            .field("airspace", &self.airspace)
            .field("vault", &self.vault)
            .field("fee_destination", &self.fee_destination)
            .field("deposit_note_mint", &self.deposit_note_mint)
            .field("loan_note_mint", &self.loan_note_mint)
            .field("token_mint", &self.token_mint)
            .field("address", &self.address)
            .field("config", &self.config)
            .field("borrowed_tokens", &self.total_borrowed())
            .field("uncollected_fees", &self.total_uncollected_fees())
            .field("deposit_tokens", &self.deposit_tokens)
            .field("deposit_notes", &self.deposit_notes)
            .field("loan_notes", &self.loan_notes)
            .field("accrued_until", &self.accrued_until)
            .field("token_price_oracle", &self.token_price_oracle)
            .finish()
    }
}

#[cfg(any(test, feature = "no-entrypoint"))]
impl Serialize for MarginPool {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("MarginPool", 13)?;
        s.serialize_field("version", &self.version)?;
        s.serialize_field("vault", &self.vault.to_string())?;
        s.serialize_field("feeDestination", &self.fee_destination.to_string())?;
        s.serialize_field("depositNoteMint", &self.deposit_note_mint.to_string())?;
        s.serialize_field("loanNoteMint", &self.loan_note_mint.to_string())?;
        s.serialize_field("tokenMint", &self.token_mint.to_string())?;
        s.serialize_field("borrowedTokens", &self.total_borrowed().to_string())?;
        s.serialize_field(
            "uncollectedFees",
            &self.total_uncollected_fees().to_string(),
        )?;
        s.serialize_field("depositTokens", &self.deposit_tokens)?;
        s.serialize_field("depositNotes", &self.deposit_notes)?;
        s.serialize_field("loanNotes", &self.loan_notes)?;
        s.serialize_field("accruedUntil", &self.accrued_until)?;
        // s.serialize_field("tokenPriceOracle", &self.token_price_oracle.to_string())?;
        s.end()
    }
}

impl MarginPool {
    /// Get the seeds needed to sign for the vault
    pub fn signer_seeds(&self) -> Result<[&[u8]; 3]> {
        if self.flags().contains(PoolFlags::DISABLED) {
            msg!("the pool is currently disabled");
            return err!(ErrorCode::Disabled);
        }

        Ok([
            self.airspace.as_ref(),
            self.token_mint.as_ref(),
            self.pool_bump.as_ref(),
        ])
    }

    /// Record a deposit into the pool
    pub fn deposit(&mut self, amount: &FullAmount) -> Result<()> {
        self.deposit_tokens = self.deposit_tokens.checked_add(amount.tokens).unwrap();
        if self.deposit_tokens > self.config.deposit_limit {
            msg!(
                "tried to deposit {} but limit is {}",
                self.deposit_tokens,
                self.config.deposit_limit
            );
            return err!(ErrorCode::DepositLimitReached);
        }
        self.deposit_notes = self.deposit_notes.checked_add(amount.notes).unwrap();
        Ok(())
    }

    /// Record a withdrawal from the pool
    pub fn withdraw(&mut self, amount: &FullAmount) -> Result<()> {
        self.deposit_tokens = self
            .deposit_tokens
            .checked_sub(amount.tokens)
            .ok_or(ErrorCode::InsufficientLiquidity)?;
        self.deposit_notes = self
            .deposit_notes
            .checked_sub(amount.notes)
            .ok_or(ErrorCode::InsufficientLiquidity)?;

        Ok(())
    }

    /// Record a loan from the pool
    pub fn borrow(&mut self, amount: &FullAmount) -> Result<()> {
        if !self.flags().contains(PoolFlags::ALLOW_LENDING) {
            msg!("this pool only allows deposits");
            return err!(ErrorCode::DepositsOnly);
        }

        self.deposit_tokens = self
            .deposit_tokens
            .checked_sub(amount.tokens)
            .ok_or(ErrorCode::InsufficientLiquidity)?;
        self.loan_notes = self.loan_notes.checked_add(amount.notes).unwrap();

        *self.total_borrowed_mut() += Number::from(amount.tokens);

        if self.total_borrowed() > &Number::from(self.config.borrow_limit) {
            return err!(ErrorCode::BorrowLimitReached);
        }

        if self.utilization_rate().as_u64(BPS_EXPONENT) > MAX_POOL_UTIL_RATIO_AFTER_BORROW_BPS {
            return Err(ErrorCode::ExceedsMaxBorrowUtilRatio.into());
        }

        Ok(())
    }

    /// Record a repayment of a loan
    pub fn repay(&mut self, amount: &FullAmount) -> Result<()> {
        self.deposit_tokens = self.deposit_tokens.checked_add(amount.tokens).unwrap();
        self.loan_notes = self
            .loan_notes
            .checked_sub(amount.notes)
            .ok_or(ErrorCode::InsufficientLiquidity)?;

        // Due to defensive rounding, and probably only when the final outstanding loan in a pool
        // is being repaid, it is possible that the integer number of tokens being repaid exceeds
        // the precise number of total borrowed tokens. To cover this case, we guard against any
        // difference beyond the rounding effect, and use a saturating sub to update the total borrowed.

        if self.total_borrowed().as_u64_ceil(0) < amount.tokens {
            return Err(ErrorCode::RepaymentExceedsTotalOutstanding.into());
        }

        *self.total_borrowed_mut() = self
            .total_borrowed()
            .saturating_sub(Number::from(amount.tokens));

        Ok(())
    }

    /// Record repayment of a loan with deposit notes.
    ///
    /// This is a kind of settlement operation where no tokens are involved explicitly. We're
    /// cancelling deposit notes against loan notes.
    pub fn margin_repay(
        &mut self,
        repay_amount: &FullAmount,
        withdraw_amount: &FullAmount,
    ) -> Result<()> {
        // "Withdraw"
        self.deposit_notes = self
            .deposit_notes
            .checked_sub(withdraw_amount.notes)
            .ok_or(ErrorCode::InsufficientLiquidity)?;

        // "Repay"
        self.loan_notes = self
            .loan_notes
            .checked_sub(repay_amount.notes)
            .ok_or(ErrorCode::InsufficientLiquidity)?;

        // Due to defensive rounding, and probably only when the final outstanding loan in a pool
        // is being repaid, it is possible that the integer number of tokens being repaid exceeds
        // the precise number of total borrowed tokens. To cover this case, we guard against any
        // difference beyond the rounding effect, and use a saturating sub to update the total borrowed.

        if self.total_borrowed().as_u64_ceil(0) < repay_amount.tokens {
            return Err(ErrorCode::RepaymentExceedsTotalOutstanding.into());
        }

        *self.total_borrowed_mut() = self
            .total_borrowed()
            .saturating_sub(Number::from(repay_amount.tokens));

        Ok(())
    }

    /// Accrue interest charges on outstanding borrows
    ///
    /// Returns true if the interest was fully accumulated, false if it was
    /// only partially accumulated (due to significant time drift).
    pub fn accrue_interest(&mut self, time: UnixTimestamp) -> bool {
        let time_behind = time - self.accrued_until;
        let time_to_accrue = std::cmp::min(time_behind, util::MAX_ACCRUAL_SECONDS);

        match time_to_accrue.cmp(&0) {
            Ordering::Less => panic!("Interest may not be accrued over a negative time period."),
            Ordering::Equal => true,
            Ordering::Greater => {
                let interest_rate = self.interest_rate();
                let compound_rate = util::compound_interest(interest_rate, time_to_accrue);

                let interest_fee_rate = Number::from_bps(self.config.management_fee_rate);
                let new_interest_accrued = *self.total_borrowed() * compound_rate;
                let fee_to_collect = new_interest_accrued * interest_fee_rate;

                *self.total_borrowed_mut() += new_interest_accrued;
                *self.total_uncollected_fees_mut() += fee_to_collect;

                self.accrued_until = self.accrued_until.checked_add(time_to_accrue).unwrap();

                time_behind == time_to_accrue
            }
        }
    }

    /// Gets the current interest rate for loans from this pool
    pub fn interest_rate(&self) -> Number {
        let borrow_0 = Number::from_bps(self.config.borrow_rate_0);

        // Catch the edge case of empty pool
        if self.deposit_notes == 0 {
            return borrow_0;
        }

        let util_rate = self.utilization_rate();

        let borrow_1 = Number::from_bps(self.config.borrow_rate_1);
        let util_1 = Number::from_bps(self.config.utilization_rate_1);

        if util_rate <= util_1 {
            // First regime
            return util::interpolate(util_rate, Number::ZERO, util_1, borrow_0, borrow_1);
        }

        let util_2 = Number::from_bps(self.config.utilization_rate_2);
        let borrow_2 = Number::from_bps(self.config.borrow_rate_2);

        if util_rate <= util_2 {
            // Second regime
            return util::interpolate(util_rate, util_1, util_2, borrow_1, borrow_2);
        }

        let borrow_3 = Number::from_bps(self.config.borrow_rate_3);

        if util_rate < Number::ONE {
            // Third regime
            return util::interpolate(util_rate, util_2, Number::ONE, borrow_2, borrow_3);
        }

        // Maximum interest
        borrow_3
    }

    /// Gets the current utilization rate of the pool
    pub fn utilization_rate(&self) -> Number {
        *self.total_borrowed() / self.total_value()
    }

    /// Collect any fees accumulated from interest
    ///
    /// Returns the number of notes to mint to represent the collected fees
    pub fn collect_accrued_fees(&mut self) -> u64 {
        let uncollected = *self.total_uncollected_fees();

        let fee_notes = (uncollected / self.deposit_note_exchange_rate()).as_u64(0);

        // Collect fees, preserving the remainder token amount.
        if fee_notes > 0 {
            let collected_tokens =
                Number::from_decimal(fee_notes, 0) * self.deposit_note_exchange_rate();
            let remainder = uncollected - collected_tokens;

            *self.total_uncollected_fees_mut() = remainder;
            self.deposit_notes = self.deposit_notes.checked_add(fee_notes).unwrap();
        }

        fee_notes
    }

    /// Calculate the prices for the deposit and loan notes, based on
    /// the price of the underlying token.
    /// Note: we can convert oracle_account to an enum if we add other oracle types.
    ///
    /// We pass in the clock as a variable as we also use this function off-chain,
    /// where we don't have access to the sysvars.
    pub fn calculate_prices(
        &self,
        update: &PriceUpdateV2,
        quote_update: Option<&PriceUpdateV2>,
        clock: &Clock,
    ) -> Result<PriceResult> {
        let (price_value, conf_value, ema_value, exponent, publish_time) =
            match self.token_price_oracle {
                TokenPriceOracle::PythPull { feed_id } => {
                    let price =
                        update.get_price_no_older_than(clock, MAX_ORACLE_STALENESS, &feed_id)?;
                    (
                        Number::from_decimal(price.price, price.exponent),
                        Number::from_decimal(price.conf, price.exponent),
                        Number::from_decimal(update.price_message.ema_price, price.exponent),
                        price.exponent,
                        price.publish_time,
                    )
                }
                TokenPriceOracle::PythPullRedemption {
                    feed_id,
                    quote_feed_id,
                } => {
                    // The quote mint should match the oracle price mint
                    let quote_update = quote_update.ok_or(ErrorCode::InvalidPoolOracle)?;
                    let price =
                        update.get_price_no_older_than(clock, MAX_ORACLE_STALENESS, &feed_id)?;
                    let quote = quote_update.get_price_no_older_than(
                        clock,
                        MAX_ORACLE_STALENESS,
                        &quote_feed_id,
                    )?;

                    // SECURITY: If we were to incorrectly configure the oracle feed chain, we could significantly misprice tokens.
                    // E.g. SUSD redemption * BTC underlying.
                    let quote_price = Number::from_decimal(quote.price, quote.exponent);
                    let quote_ema =
                        Number::from_decimal(quote_update.price_message.ema_price, quote.exponent);
                    let quote_conf = Number::from_decimal(quote.conf, quote.exponent);
                    let publish_time = price.publish_time.min(quote.publish_time);

                    // The confidence of the price is the sum of the two confidence values in USD.
                    // (quote.conf * price) + price.conf
                    (
                        Number::from_decimal(price.price, price.exponent) * quote_price,
                        Number::from_decimal(price.conf, price.exponent) * quote_price + quote_conf,
                        Number::from_decimal(update.price_message.ema_price, price.exponent)
                            * quote_ema,
                        price.exponent,
                        publish_time,
                    )
                }
                TokenPriceOracle::NoOracle => {
                    return err!(ErrorCode::InvalidPoolOracle);
                }
            };

        let deposit_note_exchange_rate = self.deposit_note_exchange_rate();
        let loan_note_exchange_rate = self.loan_note_exchange_rate();

        let deposit_note_price =
            i64::try_from((price_value * deposit_note_exchange_rate).as_u64_rounded(exponent))
                .unwrap();
        let deposit_note_conf = (conf_value * deposit_note_exchange_rate).as_u64_rounded(exponent);
        let deposit_note_twap =
            i64::try_from((ema_value * deposit_note_exchange_rate).as_u64_rounded(exponent))
                .unwrap();
        let loan_note_price =
            i64::try_from((price_value * loan_note_exchange_rate).as_u64_rounded(exponent))
                .unwrap();
        let loan_note_conf = (conf_value * loan_note_exchange_rate).as_u64_rounded(exponent);
        let loan_note_twap =
            i64::try_from((ema_value * loan_note_exchange_rate).as_u64_rounded(exponent)).unwrap();

        Ok(PriceResult {
            deposit_note_price,
            deposit_note_conf,
            deposit_note_twap,
            loan_note_price,
            loan_note_conf,
            loan_note_twap,
            publish_time,
            exponent,
        })
    }

    pub fn calculate_full_amount(
        &self,
        source_balance: FullAmount,
        destination_balance: FullAmount,
        change: TokenChange,
        action: PoolAction,
    ) -> Result<FullAmount> {
        match change.kind {
            ChangeKind::ShiftBy => self.convert_amount(Amount::tokens(change.tokens), action),
            // A user has 110 tokens when 1 token = 1.1 notes. They choose to change the source by
            // 55 tokens in either direction.
            ChangeKind::SetSourceTo | ChangeKind::SetDestinationTo => self.calculate_set_amount(
                change.kind,
                source_balance,
                destination_balance,
                Amount::tokens(change.tokens),
                action,
            ),
        }
    }

    /// Calculate the amount to change by to set either the source or destination to an amount.
    ///
    fn calculate_set_amount(
        &self,
        change_kind: ChangeKind,
        source_balance: FullAmount,
        destination_balance: FullAmount,
        target_amount: Amount,
        pool_action: PoolAction,
    ) -> Result<FullAmount> {
        match (pool_action, change_kind) {
            (_, ChangeKind::ShiftBy) => Err(ErrorCode::InvalidSetTo)?,
            (PoolAction::Borrow, ChangeKind::SetDestinationTo) => {
                // Source:      Pool Loan
                // Destination: Pool Deposit
                // Borrow an incremental amount to set destination to exactly these tokens
                // Useful when wanting to take action with an exact amount in.
                // We need to know the number of notes in the destination, and how many
                // notes are needed from the pool reserve to be lent out.

                // Let's say the destination has 100 notes, which are 105 tokens.
                // The source has 1000 notes, which are 1100 tokens.
                // We would like to set the destination to have 500 tokens, so we need
                // 395 more.
                // Do we calculate this here or before here?
                let tokens_to_borrow = target_amount
                    .value
                    .checked_sub(destination_balance.tokens)
                    .ok_or(ErrorCode::SetMathOp)?;
                self.convert_amount(Amount::tokens(tokens_to_borrow), PoolAction::Borrow)
            }
            (PoolAction::Deposit, ChangeKind::SetDestinationTo) => {
                // We are setting the pool deposit account to the target amount by withdrawing tokens from the token account
                let current_destination_tokens = destination_balance.tokens;
                let difference_in_tokens = target_amount
                    .value
                    .checked_sub(current_destination_tokens)
                    .ok_or(ErrorCode::SetMathOp)?;
                self.convert_amount(Amount::tokens(difference_in_tokens), PoolAction::Deposit)
            }
            (PoolAction::Repay, ChangeKind::SetDestinationTo) => {
                // We are repaying a loan using a deposit. This is the inverse of Borrow above.
                let current_destination_tokens = destination_balance.tokens;
                let difference_in_tokens = current_destination_tokens
                    .checked_sub(target_amount.value)
                    .ok_or(ErrorCode::SetMathOp)?;
                self.convert_amount(Amount::tokens(difference_in_tokens), PoolAction::Repay)
            }
            (PoolAction::Withdraw, ChangeKind::SetDestinationTo) => {
                // Set the token account balnace to the target amount by reducing the deposit note account
                let current_destination_tokens = destination_balance.tokens;
                let difference_in_tokens = target_amount
                    .value
                    .checked_sub(current_destination_tokens)
                    .ok_or(ErrorCode::SetMathOp)?;
                self.convert_amount(Amount::tokens(difference_in_tokens), PoolAction::Withdraw)
            }
            (PoolAction::Borrow, ChangeKind::SetSourceTo) => {
                // Borrow an amount that gets the deposit note to have a certain value
                let current_source_tokens = source_balance.tokens;
                let difference_in_tokens = target_amount
                    .value
                    .checked_sub(current_source_tokens)
                    .ok_or(ErrorCode::SetMathOp)?;
                self.convert_amount(Amount::tokens(difference_in_tokens), PoolAction::Borrow)
            }
            (PoolAction::Deposit, ChangeKind::SetSourceTo) => {
                // Source:      User Token
                // Destination: Pool Deposit
                // Set the source token account to this amount and deposit the difference into the pool
                let difference_in_tokens = source_balance
                    .tokens
                    .checked_sub(target_amount.value)
                    .ok_or(ErrorCode::SetMathOp)?;
                self.convert_amount(Amount::tokens(difference_in_tokens), PoolAction::Deposit)
            }
            (PoolAction::Repay, ChangeKind::SetSourceTo) => {
                // Source:      Pool Deposit
                // Destination: Pool Loan
                // The set amount should be the lesser of the borrowed amount and the amount
                // withdrawn from the source
                let borrowed_amount = destination_balance.tokens;
                let drain_by_amount = source_balance
                    .tokens
                    .checked_sub(target_amount.value)
                    .ok_or(ErrorCode::SetMathOp)?;
                self.convert_amount(
                    Amount::tokens(borrowed_amount.min(drain_by_amount)),
                    PoolAction::Repay,
                )
            }
            (PoolAction::Withdraw, ChangeKind::SetSourceTo) => {
                // Source:      Pool Deposit
                // Destination: User Token
                // Withdraw and leave the pool deposit with the target balance
                let difference_in_tokens = source_balance
                    .tokens
                    .checked_sub(target_amount.value)
                    .ok_or(ErrorCode::SetMathOp)?;
                // Amount to be withdrawn from the source
                self.convert_amount(Amount::tokens(difference_in_tokens), PoolAction::Withdraw)
            }
        }
    }

    /// Convert the `Amount` to a `FullAmount` conisting of the appropriate proprtion of notes and tokens
    pub fn convert_amount(&self, amount: Amount, action: PoolAction) -> Result<FullAmount> {
        let (exchange_rate, rounding) = match action {
            PoolAction::Deposit | PoolAction::Withdraw => (
                self.deposit_note_exchange_rate(),
                RoundingDirection::direction(action, amount.kind),
            ),
            PoolAction::Repay | PoolAction::Borrow => (
                self.loan_note_exchange_rate(),
                RoundingDirection::direction(action, amount.kind),
            ),
        };

        let amount = Self::convert_with_rounding_and_rate(amount, rounding, exchange_rate);

        // As FullAmount represents the conversion of tokens to/from notes for
        // the purpose of:
        // - adding/subtracting tokens to/from a pool's vault
        // - minting/burning notes from a pool's deposit/loan mint.
        // There should be no scenario where a conversion between notes and tokens
        // leads to either value being 0 while the other is not.
        //
        // Scenarios where this can happen could be security risks, such as:
        // - A user withdraws 1 token but burns 0 notes, they are draining the pool.
        // - A user deposits 1 token but mints 0 notes, they are losing funds for no value.
        // - A user deposits 0 tokens but mints 1 notes, they are getting free deposits.
        // - A user withdraws 0 tokens but burns 1 token, they are writing off debt.
        //
        // Thus we finally check that both values are positive.
        if (amount.notes == 0 && amount.tokens > 0) || (amount.tokens == 0 && amount.notes > 0) {
            return err!(crate::ErrorCode::InvalidAmount);
        }

        Ok(amount)
    }

    /// Isolated to ensure rounding implementation
    fn convert_with_rounding_and_rate(
        amount: Amount,
        rounding: RoundingDirection,
        exchange_rate: Number,
    ) -> FullAmount {
        match amount.kind {
            AmountKind::Tokens => FullAmount {
                tokens: amount.value,
                notes: match rounding {
                    RoundingDirection::Down => {
                        (Number::from(amount.value) / exchange_rate).as_u64(0)
                    }
                    RoundingDirection::Up => {
                        (Number::from(amount.value) / exchange_rate).as_u64_ceil(0)
                    }
                },
            },

            AmountKind::Notes => FullAmount {
                notes: amount.value,
                tokens: match rounding {
                    RoundingDirection::Down => {
                        (Number::from(amount.value) * exchange_rate).as_u64(0)
                    }
                    RoundingDirection::Up => {
                        (Number::from(amount.value) * exchange_rate).as_u64_ceil(0)
                    }
                },
            },
        }
    }

    /// Get the exchange rate for deposit note -> token.
    /// If the pool is only left with uncollected fees (deposit notes = 0), then
    /// the deposit note exchange rate is 1.
    pub fn deposit_note_exchange_rate(&self) -> Number {
        let deposit_notes = std::cmp::max(1, self.deposit_notes);
        let total_value = std::cmp::max(
            Number::ONE,
            self.total_value() - *self.total_uncollected_fees(),
        );
        total_value / Number::from(deposit_notes)
    }

    /// Get the exchange rate for loan note -> token
    pub fn loan_note_exchange_rate(&self) -> Number {
        let loan_notes = std::cmp::max(1, self.loan_notes);
        let total_borrowed = std::cmp::max(Number::ONE, *self.total_borrowed());
        total_borrowed / Number::from(loan_notes)
    }

    /// Gets the total value of assets owned by/owed to the pool.
    fn total_value(&self) -> Number {
        *self.total_borrowed() + Number::from(self.deposit_tokens)
    }

    fn total_uncollected_fees_mut(&mut self) -> &mut Number {
        bytemuck::from_bytes_mut(&mut self.uncollected_fees)
    }

    pub fn total_uncollected_fees(&self) -> &Number {
        bytemuck::from_bytes(&self.uncollected_fees)
    }

    fn total_borrowed_mut(&mut self) -> &mut Number {
        bytemuck::from_bytes_mut(&mut self.borrowed_tokens)
    }

    pub fn total_borrowed(&self) -> &Number {
        bytemuck::from_bytes(&self.borrowed_tokens)
    }

    fn flags(&self) -> PoolFlags {
        PoolFlags::from_bits_truncate(self.config.flags)
    }
}

#[derive(Debug, Default)]
pub struct FullAmount {
    pub tokens: u64,
    pub notes: u64,
}

/// Represents the primary pool actions, used in determining the
/// rounding direction between tokens and notes.
#[derive(Clone, Copy, Debug)]
pub enum PoolAction {
    Borrow,
    Deposit,
    Repay,
    Withdraw,
}

/// Represents the direction in which we should round when converting
/// between tokens and notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundingDirection {
    Down,
    Up,
}

impl RoundingDirection {
    /// The exchange rate increases over time due to interest.
    /// The rate is notes:tokens, such that 1.2 means that 1 note = 1.2 tokens.
    /// This is because a user deposits 1 token and gets 1 note back (assuming 1:1 rate),
    /// they then earn interest due passage of time, and become entitled to
    /// 1.2 tokens, where 0.2 is the interest. Thus 1 note becomes 1.2 tokens.
    ///
    /// In an exchange where a user supplies notes, we multiply by the exchange rate
    /// to get tokens.
    /// In an exchange where a user supplies tokens, we divide by the exchange rate
    /// to get notes.
    ///
    /// `amount` can either be tokens or notes. The amount type (1), side of the position
    /// in the pool (2), and the instruction type (3), impact the rounding direction.
    /// We always want a rounding position that is favourable to the pool.
    /// The combination of the 3 factors is shown in the table below.
    ///
    /// | Instruction | Note Action     | Direction      | Rounding |
    /// | :---        |     :----:      |     :----:     |     ---: |
    /// | Deposit     | Mint Collateral | Tokens > Notes | Down     |
    /// | Deposit     | Mint Collateral | Notes > Tokens | Up       |
    /// | Withdraw    | Burn Collateral | Tokens > Notes | Up       |
    /// | Withdraw    | Burn Collateral | Notes > Tokens | Down     |
    /// | Borrow      | Mint Claim      | Tokens > Notes | Up       |
    /// | Borrow      | Mint Claim      | Notes > Tokens | Down     |
    /// | Repay       | Burn Claim      | Tokens > Notes | Down     |
    /// | Repay       | Burn Claim      | Notes > Tokens | Up       |
    pub const fn direction(pool_action: PoolAction, amount_kind: AmountKind) -> Self {
        use RoundingDirection::*;
        match (pool_action, amount_kind) {
            (PoolAction::Borrow, AmountKind::Tokens)
            | (PoolAction::Deposit, AmountKind::Notes)
            | (PoolAction::Repay, AmountKind::Notes)
            | (PoolAction::Withdraw, AmountKind::Tokens) => Up,
            (PoolAction::Borrow, AmountKind::Notes)
            | (PoolAction::Deposit, AmountKind::Tokens)
            | (PoolAction::Repay, AmountKind::Tokens)
            | (PoolAction::Withdraw, AmountKind::Notes) => Down,
        }
    }
}

pub struct PriceResult {
    pub deposit_note_price: i64,
    pub deposit_note_conf: u64,
    pub deposit_note_twap: i64,
    pub loan_note_price: i64,
    pub loan_note_conf: u64,
    pub loan_note_twap: i64,
    pub publish_time: i64,
    pub exponent: i32,
}

/// Configuration for a margin pool
#[derive(Debug, Default, AnchorDeserialize, AnchorSerialize, Clone, Copy, Eq, PartialEq)]
#[cfg_attr(any(feature = "no-entrypoint", test), derive(Serialize, Deserialize))]
pub struct MarginPoolConfig {
    /// Space for binary settings
    pub flags: u64,

    /// The utilization rate at which first regime transitions to second
    pub utilization_rate_1: u16,

    /// The utilization rate at which second regime transitions to third
    pub utilization_rate_2: u16,

    /// The lowest borrow rate
    pub borrow_rate_0: u16,

    /// The borrow rate at the transition point from first to second regime
    pub borrow_rate_1: u16,

    /// The borrow rate at the transition point from second to third regime
    pub borrow_rate_2: u16,

    /// The highest possible borrow rate.
    pub borrow_rate_3: u16,

    /// The fee rate applied to interest payments collected
    pub management_fee_rate: u16,

    /// The limit of tokens that can be deposited into the pool
    pub deposit_limit: u64,

    /// The limit of tokens that can be borrowed from the pool
    pub borrow_limit: u64,

    /// Unused
    #[cfg_attr(feature = "no-entrypoint", serde(default))]
    pub reserved: u64,
}

bitflags::bitflags! {
    pub struct PoolFlags: u64 {
        /// The pool is not allowed to sign for anything, preventing
        /// the movement of funds.
        const DISABLED = 1 << 0;

        /// The pool is allowed to lend out deposits for borrowing
        const ALLOW_LENDING = 1 << 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_test::{assert_ser_tokens, Token};

    // Helper method to replicate the essential deposit_handler logic up until pool deposit
    fn check_deposit_amount_action_deposit(
        pool: &MarginPool,
        pool_token_balance: u64,
        source_token_amount: u64,
        dst_pool_notes: u64,
        expected: FullAmount,
    ) -> Result<()> {
        let change = TokenChange {
            kind: ChangeKind::SetDestinationTo,
            tokens: pool_token_balance,
        };
        let source_balance =
            pool.convert_amount(Amount::tokens(source_token_amount), PoolAction::Deposit)?;
        let destination_balance =
            pool.convert_amount(Amount::notes(dst_pool_notes), PoolAction::Deposit)?;

        let deposit_amount = pool.calculate_full_amount(
            source_balance,
            destination_balance,
            change,
            PoolAction::Deposit,
        )?;

        assert_eq!(deposit_amount.tokens, expected.tokens);
        assert_eq!(deposit_amount.notes, expected.notes);

        Ok(())
    }

    #[test]
    fn test_deposit_flow_simple_increase() -> Result<()> {
        let mut pool = MarginPool::default();
        pool.config.deposit_limit = 2_000_000;

        let amount = 2_000_000;
        let src_token_amount = 1_000_000;
        let dst_pool_notes = 1_000_000;
        let expected = FullAmount {
            tokens: 1_000_000,
            notes: 1_000_000,
        };

        // Assert the deposit amount calculation
        check_deposit_amount_action_deposit(
            &pool,
            amount,
            src_token_amount,
            dst_pool_notes,
            expected,
        )
        .unwrap();

        // Inject the pool with initial tokens and notes
        pool.deposit(&FullAmount {
            tokens: 1_000_000,
            notes: 1_000_000,
        })?;

        // Simulate interest rate accrual:
        // new interest rate is then:
        // (1_000_000 + 150_000 / 1_000_000) = 1.15
        *pool.total_borrowed_mut() += Number::from(150_000);

        let full = pool.convert_amount(Amount::tokens(1_150_000), PoolAction::Deposit)?;
        assert_eq!(full.tokens, 1_150_000);
        assert_eq!(full.notes, 1_000_000);

        Ok(())
    }

    #[test]
    fn test_deposit_flow_simple_decrease() -> Result<()> {
        let mut pool = MarginPool::default();
        pool.config.deposit_limit = 2_000_000;

        // Initial deposit of 1M tokens and notes (rate = 1.0)
        pool.deposit(&FullAmount {
            tokens: 1_000_000,
            notes: 1_000_000,
        })?;

        // Inflate notes to bad rate
        pool.deposit_tokens += 100_000; // 1.1M total
        pool.deposit_notes += 1_000_000; // 2M total

        // Rate is now: 1.1/2 = 0.55
        let rate = pool.deposit_note_exchange_rate();
        let expected_rate = Number::from(1_100_000) / Number::from(2_000_000);
        assert_eq!(rate, expected_rate);

        Ok(())
    }

    #[test]
    fn test_max_borrow_constraint_ok() -> Result<()> {
        let mut margin_pool = MarginPool::default();
        margin_pool.config.flags = PoolFlags::ALLOW_LENDING.bits();
        margin_pool.config.borrow_limit = u64::MAX;
        margin_pool.config.deposit_limit = u64::MAX;

        margin_pool.deposit(&FullAmount {
            tokens: 1_000_000,
            notes: 1_000_000,
        })?;

        // Assumes MAX_POOL_UTIL_RATIO_AFTER_BORROW_BPS == 95 bps
        margin_pool.borrow(&FullAmount {
            tokens: 950_000,
            notes: 855_000,
        })?;

        Ok(())
    }

    #[test]
    fn test_max_borrow_constraint_err() -> Result<()> {
        let mut margin_pool = MarginPool::default();
        margin_pool.config.flags = PoolFlags::ALLOW_LENDING.bits();
        margin_pool.config.borrow_limit = u64::MAX;
        margin_pool.config.deposit_limit = u64::MAX;

        margin_pool.deposit(&FullAmount {
            tokens: 1_000_000,
            notes: 1_000_000,
        })?;

        // Assumes MAX_POOL_UTIL_RATIO_AFTER_BORROW_BPS == 95 bps
        assert_eq!(
            margin_pool
                .borrow(&FullAmount {
                    tokens: 950_100,
                    notes: 855_090,
                })
                .unwrap_err(),
            ErrorCode::ExceedsMaxBorrowUtilRatio.into()
        );

        Ok(())
    }

    #[test]
    fn test_deposit_note_rounding() -> Result<()> {
        let mut margin_pool = MarginPool::default();
        margin_pool.config.borrow_limit = u64::MAX;
        margin_pool.config.deposit_limit = u64::MAX;

        margin_pool.deposit(&FullAmount {
            tokens: 1_000_000,
            notes: 900_000,
        })?;

        // Deposit note exchange rate is 1.111111_.
        // If a user withdraws 9 notes, they should get 9 or 10 tokens back
        // depending on the rounding.

        assert_eq!(
            margin_pool.deposit_note_exchange_rate().as_u64(-9),
            1111111111
        );

        let pool_convert = |amount, rounding| {
            let exchange_rate = margin_pool.deposit_note_exchange_rate();
            MarginPool::convert_with_rounding_and_rate(amount, rounding, exchange_rate)
        };

        let deposit_amount = pool_convert(Amount::notes(12), RoundingDirection::Down);

        assert_eq!(deposit_amount.notes, 12);
        assert_eq!(deposit_amount.tokens, 13); // ref [0]

        let deposit_amount = pool_convert(Amount::notes(18), RoundingDirection::Down);

        assert_eq!(deposit_amount.notes, 18);
        assert_eq!(deposit_amount.tokens, 19);

        let deposit_amount = pool_convert(Amount::notes(12), RoundingDirection::Up);

        assert_eq!(deposit_amount.notes, 12);
        assert_eq!(deposit_amount.tokens, 14); // ref [1]

        // A user requesting 1 note should never get 0 tokens back,
        // or 1 token should never get 0 notes back

        let deposit_amount = pool_convert(Amount::notes(1), RoundingDirection::Down);

        // When depositing, 1:1 would be advantageous to the user
        assert_eq!(deposit_amount.notes, 1);
        assert_eq!(deposit_amount.tokens, 1);

        let deposit_amount = pool_convert(Amount::notes(1), RoundingDirection::Up);

        // Depositing 2 tokens for 1 note is disadvantageous to the user
        // and protects the protocol's average exchange rate
        assert_eq!(deposit_amount.notes, 1);
        assert_eq!(deposit_amount.tokens, 2);

        // Check the default rounding for depositing notes, as it is disadvantageous
        // to the user per the previous observation.
        let direction = RoundingDirection::direction(PoolAction::Deposit, AmountKind::Notes);
        assert_eq!(RoundingDirection::Up, direction);

        // A repay is the same as a deposit (inflow)
        let direction = RoundingDirection::direction(PoolAction::Repay, AmountKind::Notes);
        assert_eq!(RoundingDirection::Up, direction);

        Ok(())
    }

    // Error cases where deposits and borrows exceeds the configurated limit
    #[test]
    fn test_deposit_and_borrow_exceeding_limit_error() -> Result<()> {
        let mut margin_pool = MarginPool::default();
        margin_pool.config.flags = PoolFlags::ALLOW_LENDING.bits();
        margin_pool.config.borrow_limit = 100_000;
        margin_pool.config.deposit_limit = 250_000;

        let result = margin_pool.deposit(&FullAmount {
            tokens: 250_001,
            notes: 200_000,
        });

        assert!(result.is_err());
        assert!(result.err().unwrap() == ErrorCode::DepositLimitReached.into());

        let result = margin_pool.borrow(&FullAmount {
            tokens: 100_001,
            notes: 100_000,
        });

        assert!(result.is_err());
        assert!(result.err().unwrap() == ErrorCode::BorrowLimitReached.into());

        Ok(())
    }

    /// Conversion between tokens and notes would allow a user to
    /// provide tokens for notes, or to specify the number of tokens
    /// to receive on withdrawal.
    ///
    /// As the exchange rate between notes and tokens is expected to
    /// increase over time, there is a risk that a user could extract
    /// 1 token while burning 0 notes due to rounding.
    #[test]
    fn test_deposit_token_rounding() -> Result<()> {
        let mut margin_pool = MarginPool::default();
        margin_pool.config.borrow_limit = u64::MAX;
        margin_pool.config.deposit_limit = u64::MAX;

        margin_pool.deposit(&FullAmount {
            tokens: 1_000_000,
            notes: 900_000,
        })?;

        assert_eq!(
            margin_pool.deposit_note_exchange_rate().as_u64(-9),
            1111111111
        );

        let pool_convert = |amount, rounding| {
            let exchange_rate = margin_pool.deposit_note_exchange_rate();
            MarginPool::convert_with_rounding_and_rate(amount, rounding, exchange_rate)
        };

        // depositing tokens should round down
        let deposit_result = margin_pool.convert_amount(Amount::tokens(1), PoolAction::Deposit);

        // Rounding down would return 0 notes
        assert!(deposit_result.is_err());

        let deposit_amount = pool_convert(Amount::tokens(1), RoundingDirection::Up);

        // Depositing 1 token for 1 note is disadvantageous to the user as they
        // get a lower rate than the 1.111_.
        // This is however because they are requesting the smallest unit, so
        // this test hides the true intention of the rounding.
        assert_eq!(deposit_amount.notes, 1);
        assert_eq!(deposit_amount.tokens, 1);

        // It is better observed with a bigger number.
        // The expectation when a user deposits is that they should get less notes
        // than the exchange rate if we have to round. This is because fewer notes
        // entitle the user to fewer tokens on withdrawal from the pool.

        // We start by rounding up a bigger number. See [0]
        let deposit_amount = pool_convert(Amount::tokens(9), RoundingDirection::Up);

        assert_eq!(deposit_amount.notes, 9);
        assert_eq!(deposit_amount.tokens, 9);

        // [1] shows the behaviour when rounding 12 notes up, we get 13 tokens.
        let deposit_amount = pool_convert(Amount::tokens(13), RoundingDirection::Up);

        assert_eq!(deposit_amount.tokens, 13);
        // [1] returned 12 notes, and we get 12 notes back.
        assert_eq!(deposit_amount.notes, 12);

        // If we round down instead of up, we preserve value.
        let deposit_amount = pool_convert(Amount::tokens(14), RoundingDirection::Down);

        assert_eq!(deposit_amount.tokens, 14);
        assert_eq!(deposit_amount.notes, 12);

        // From the above scenarios, we achieve a roundtrip when we change the
        // rounding direction depending on the conversion direction.
        // When depositing notes, we rounded up. When depositing tokens, rounding
        // down leaves the user in a comparable scenario.

        // Thus when depositing tokens, we should round down.
        let direction = RoundingDirection::direction(PoolAction::Deposit, AmountKind::Tokens);
        assert_eq!(RoundingDirection::Down, direction);

        // Repay should behave like deposit
        let direction = RoundingDirection::direction(PoolAction::Repay, AmountKind::Tokens);
        assert_eq!(RoundingDirection::Down, direction);

        Ok(())
    }

    #[test]
    fn test_loan_note_rounding() -> Result<()> {
        let mut margin_pool = MarginPool::default();
        margin_pool.config.flags = PoolFlags::ALLOW_LENDING.bits();
        margin_pool.config.borrow_limit = u64::MAX;
        margin_pool.config.deposit_limit = u64::MAX;

        // Deposit funds so there is liquidity
        margin_pool.deposit(&FullAmount {
            tokens: 2_000_000,
            notes: 2_000_000,
        })?;

        margin_pool.borrow(&FullAmount {
            tokens: 1_000_000,
            notes: 900_000,
        })?;

        assert_eq!(margin_pool.loan_note_exchange_rate().as_u64(-9), 1111111111);

        let pool_convert = |amount, rounding| {
            let exchange_rate = margin_pool.loan_note_exchange_rate();
            MarginPool::convert_with_rounding_and_rate(amount, rounding, exchange_rate)
        };

        let loan_amount = pool_convert(Amount::notes(1), RoundingDirection::Down);

        assert_eq!(loan_amount.notes, 1);
        assert_eq!(loan_amount.tokens, 1);

        let loan_amount = pool_convert(Amount::notes(1), RoundingDirection::Up);

        // When withdrawing, rounding up benefits the user at the cost of the
        // protocol. The user gets to borrow at a lower rate (0.5 vs 1.111_).
        assert_eq!(loan_amount.notes, 1);
        assert_eq!(loan_amount.tokens, 2);

        // Check that borrow rounding is down, so the user does not borrow at
        // a lower rate.
        let direction = RoundingDirection::direction(PoolAction::Withdraw, AmountKind::Notes);
        assert_eq!(RoundingDirection::Down, direction);

        // A borrow is the same as withdraw (outflow)
        let direction = RoundingDirection::direction(PoolAction::Borrow, AmountKind::Notes);
        assert_eq!(RoundingDirection::Down, direction);

        Ok(())
    }

    #[test]
    fn test_loan_token_rounding() -> Result<()> {
        let mut margin_pool = MarginPool::default();
        margin_pool.config.flags = PoolFlags::ALLOW_LENDING.bits();
        margin_pool.config.borrow_limit = u64::MAX;
        margin_pool.config.deposit_limit = u64::MAX;

        margin_pool.deposit(&FullAmount {
            tokens: 2_000_000,
            notes: 2_000_000,
        })?;

        margin_pool.borrow(&FullAmount {
            tokens: 1_000_000,
            notes: 900_000,
        })?;

        assert_eq!(margin_pool.loan_note_exchange_rate().as_u64(-9), 1111111111);

        let pool_convert = |amount, rounding| {
            let exchange_rate = margin_pool.loan_note_exchange_rate();
            MarginPool::convert_with_rounding_and_rate(amount, rounding, exchange_rate)
        };

        // repaying tokens rounds down
        let loan_result = margin_pool.convert_amount(Amount::tokens(1), PoolAction::Repay);

        // Rounding down to 0 is not allowed
        assert!(loan_result.is_err());

        let loan_amount = pool_convert(Amount::tokens(1), RoundingDirection::Up);

        // When withdrawing tokens, the user should get 111 tokens for 100 notes (or less)
        // at the current exchange rate. A 1:1 is disadvantageous to the user
        // as the user can borrow 111 times, and get 111 tokens for 111 notes,
        // which if they borrowed at once, they could have received more tokens.
        assert_eq!(loan_amount.notes, 1);
        assert_eq!(loan_amount.tokens, 1);

        let loan_amount = pool_convert(Amount::tokens(111), RoundingDirection::Up);

        assert_eq!(loan_amount.tokens, 111);
        // Even at a larger quantity, rounding up is still disadvantageous as
        // the user borrows at a lower rate than the prevailing exchange rate.
        assert_eq!(loan_amount.notes, 100);

        // In this instance, there is a difference in rationale between borrowing
        // and withdrawing.
        // When borrowing, we mint loan notes, and would want to mint more notes
        // for the same tokens if rounding is involved.
        let direction = RoundingDirection::direction(PoolAction::Borrow, AmountKind::Tokens);
        assert_eq!(RoundingDirection::Up, direction);

        // When withdrawing from a deposit pool, we want to give the user
        // less tokens for more notes.
        // Thus the rounding in a withdrawal from tokens should be up,
        // as 1 token would mean more notes.
        let direction = RoundingDirection::direction(PoolAction::Withdraw, AmountKind::Tokens);
        assert_eq!(RoundingDirection::Up, direction);

        Ok(())
    }

    #[test]
    fn margin_pool_serialization() {
        let pool = MarginPool::default();
        assert_ser_tokens(
            &pool,
            &[
                Token::Struct {
                    name: "MarginPool",
                    len: 13,
                },
                Token::Str("version"),
                Token::U8(0),
                Token::Str("vault"),
                Token::Str("11111111111111111111111111111111"),
                Token::Str("feeDestination"),
                Token::Str("11111111111111111111111111111111"),
                Token::Str("depositNoteMint"),
                Token::Str("11111111111111111111111111111111"),
                Token::Str("loanNoteMint"),
                Token::Str("11111111111111111111111111111111"),
                Token::Str("tokenMint"),
                Token::Str("11111111111111111111111111111111"),
                Token::Str("borrowedTokens"),
                Token::Str("0.0"),
                Token::Str("uncollectedFees"),
                Token::Str("0.0"),
                Token::Str("depositTokens"),
                Token::U64(0),
                Token::Str("depositNotes"),
                Token::U64(0),
                Token::Str("loanNotes"),
                Token::U64(0),
                Token::Str("accruedUntil"),
                Token::I64(0),
                Token::StructEnd,
            ],
        );
    }

    #[test]
    fn test_pool_rates() {
        let exponent = -6i8;
        let mut depositor = FullAmount {
            tokens: 0,
            notes: 1_000_000,
        };
        let mut borrower = FullAmount {
            tokens: 0,
            notes: 500_000,
        };
        let mut test_clock = 1_735_689_600; // 2025-01-01
        let mut pool = MarginPool {
            config: MarginPoolConfig {
                flags: 1 << 1,
                utilization_rate_1: 50,
                utilization_rate_2: 80,
                borrow_rate_0: 50,
                borrow_rate_1: 200,
                borrow_rate_2: 1000,
                borrow_rate_3: 15000,
                management_fee_rate: 20_00,
                deposit_limit: 10_000_000,
                borrow_limit: 7_000_000,
                ..Default::default()
            },
            accrued_until: test_clock,
            ..Default::default()
        };

        // Track the number of tokens that have moved in the pool
        let mut pool_tokens = 0;
        // Set the depositor and borrower starting rates
        depositor.tokens = (pool.deposit_note_exchange_rate()
            * Number::from_decimal(depositor.notes, exponent))
        .as_u64(exponent);
        assert_eq!(depositor.tokens, 1_000_000);
        assert_eq!(depositor.notes, 1_000_000);
        borrower.tokens = (pool.loan_note_exchange_rate()
            * Number::from_decimal(borrower.notes, exponent))
        .as_u64(exponent);
        assert_eq!(borrower.tokens, 500_000);
        assert_eq!(borrower.notes, 500_000);

        pool.accrued_until = test_clock; // 2025-01-01
        pool.deposit(&FullAmount {
            tokens: 1_000_000,
            notes: 1_000_000,
        })
        .unwrap();
        pool_tokens += 1_000_000;
        pool.borrow(&FullAmount {
            tokens: 500_000,
            notes: 500_000,
        })
        .unwrap();
        pool_tokens -= 500_000;
        // Accrue interest until end of 2025-01-31
        test_clock += 2_678_400; // 2025-02-01
        while !pool.accrue_interest(test_clock) {}

        // Update balanses
        depositor.tokens = (pool.deposit_note_exchange_rate()
            * Number::from_decimal(depositor.notes, exponent))
        .as_u64(exponent);
        borrower.tokens = (pool.loan_note_exchange_rate()
            * Number::from_decimal(borrower.notes, exponent))
        .as_u64(exponent);

        // The borrower balances should be the same as the pool
        assert_eq!(borrower.tokens, pool.total_borrowed().as_u64(0));
        assert_eq!(borrower.notes, pool.loan_notes);

        // Repay 500'000 loan notes originally borrowed
        pool.repay(&borrower).unwrap();
        pool_tokens += borrower.tokens; // The tokens returned by the borrower
        borrower.notes = 0;
        borrower.tokens = (pool.loan_note_exchange_rate()
            * Number::from_decimal(borrower.notes, exponent))
        .as_u64(exponent);
        // Borrower's tokens should be 0
        assert_eq!(borrower.tokens, 0);

        // Loan note exchange rate should be 1.0
        let exchange_rate = pool.loan_note_exchange_rate();
        assert_eq!(exchange_rate.as_u64(-6), 1_000_000);

        // The depositor withdraws their notes
        pool.withdraw(&depositor).unwrap();
        pool_tokens -= depositor.tokens; // The tokens taken by the depositor
        depositor.notes = 0;
        depositor.tokens = (pool.deposit_note_exchange_rate()
            * Number::from_decimal(depositor.notes, exponent))
        .as_u64(exponent);
        // Depositor's tokens should be 0
        assert_eq!(depositor.tokens, 0);

        // The remaining uncollected tokens in the pool pertain to the fees.
        assert_eq!(
            pool.total_uncollected_fees().as_u64_ceil(0),
            pool.deposit_tokens
        );

        // The deposit note exchange rate should be 1.0
        let exchange_rate = pool.deposit_note_exchange_rate();
        assert_eq!(exchange_rate.as_u64(-6), 1_000_000);

        let collected_fees = pool.collect_accrued_fees();
        // -1 because of rounding, 0.9 remains uncollected
        assert_eq!(collected_fees, pool_tokens - 1);

        assert_eq!(collected_fees, pool.deposit_notes);
        assert_eq!(pool_tokens, pool.deposit_tokens);

        // Test the exchange rates again
        assert_eq!(
            pool.deposit_note_exchange_rate().as_u64(-6),
            // The exchange rate isn't exactly 1.0 because there are uncollected
            // fees of 0.9 tokens.
            1_000_110
        );
        // The borrow rate should be exactly 1.0
        assert_eq!(pool.loan_note_exchange_rate().as_u64(-6), 1_000_000);
    }
}
