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

use anchor_lang::prelude::*;

use glow_margin::{
    AdapterResult, MarginAccount, PositionChange, PriceChangeInfo, MAX_ORACLE_STALENESS,
};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;

use crate::state::*;

#[derive(Accounts)]
pub struct MarginRefreshPosition<'info> {
    /// The margin account being executed on
    pub margin_account: AccountLoader<'info, MarginAccount>,

    /// The pool to be refreshed
    pub margin_pool: Account<'info, MarginPool>,

    /// The oracle for the token. If the oracle is a redemption rate, it should be the redemption oracle.
    /// If the oracle is not a redemption rate, it should be the price oracle.
    /// CHECK: We verify this account against the pyth pull receiver program
    pub price_oracle: AccountInfo<'info>,

    /// An optional oracle price account for the quote token, if the position uses a redemption rate.
    /// CHECK: We verify this account against the pyth pull receiver program
    pub redemption_quote_oracle: Option<AccountInfo<'info>>,
}

pub fn margin_refresh_position_handler(ctx: Context<MarginRefreshPosition>) -> Result<()> {
    #[cfg(not(feature = "testing"))]
    {
        // The account must be owned by the Pyth receiver or our test program (devnet) if not testing
        #[cfg(feature = "devnet")]
        require!(
            ctx.accounts.price_oracle.owner
                == &pubkey!("test7JXXboKpc8hGTadvoXcFWN4xgnHLGANU92JKrwA"),
            crate::ErrorCode::InvalidPoolOracle
        );
        #[cfg(not(feature = "devnet"))]
        require!(
            ctx.accounts.price_oracle.owner == &pyth_solana_receiver_sdk::id(),
            crate::ErrorCode::InvalidPoolOracle
        );
        if let Some(oracle) = &ctx.accounts.redemption_quote_oracle {
            // The account must be owned by the Pyth receiver or our test program (devnet) if not testing.
            // Anchor has a quirk where this validation is still applied against an empty optional account,
            // so we don't fail the check if the account is owned by the system program.
            #[cfg(feature = "devnet")]
            require!(
                oracle.owner == &pubkey!("test7JXXboKpc8hGTadvoXcFWN4xgnHLGANU92JKrwA")
                    || oracle.key() == Pubkey::default(),
                crate::ErrorCode::InvalidPoolOracle
            );
            #[cfg(not(feature = "devnet"))]
            require!(
                oracle.owner == &pyth_solana_receiver_sdk::id()
                    || oracle.key() == Pubkey::default(),
                crate::ErrorCode::InvalidPoolOracle
            );
        }
    }
    let clock = Clock::get()?;
    let min_oracle_freshness = clock.unix_timestamp - MAX_ORACLE_STALENESS as i64;
    let oracle_data = ctx.accounts.price_oracle.try_borrow_data()?;
    let oracle_update = PriceUpdateV2::try_deserialize(&mut &oracle_data[..])?;
    if oracle_update.price_message.publish_time < min_oracle_freshness {
        msg!("stale oracle: {}", oracle_update.price_message.publish_time);
    }

    let pool = &ctx.accounts.margin_pool;

    let quote_oracle_update = {
        if !pool.token_price_oracle.is_redemption_rate() {
            None
        } else {
            let oracle_data = ctx
                .accounts
                .redemption_quote_oracle
                .as_ref()
                .ok_or(crate::ErrorCode::MissingQuoteOracleAccount)?;
            let oracle_data = oracle_data.try_borrow_data()?;
            let update = PriceUpdateV2::try_deserialize(&mut &oracle_data[..])?;
            if update.price_message.publish_time < min_oracle_freshness {
                msg!("stale quote oracle: {}", update.price_message.publish_time);
            }
            Some(update)
        }
    };

    let prices = pool.calculate_prices(&oracle_update, quote_oracle_update.as_ref(), &clock)?;

    // Tell the margin program what the current prices are
    glow_margin::write_adapter_result(
        &*ctx.accounts.margin_account.load()?,
        &AdapterResult {
            position_changes: vec![
                (
                    pool.deposit_note_mint,
                    vec![PositionChange::Price(PriceChangeInfo::new(
                        prices.deposit_note_price,
                        prices.deposit_note_conf,
                        prices.deposit_note_twap,
                        prices.publish_time,
                        prices.exponent,
                    ))],
                ),
                (
                    pool.loan_note_mint,
                    vec![PositionChange::Price(PriceChangeInfo::new(
                        prices.loan_note_price,
                        prices.loan_note_conf,
                        prices.loan_note_twap,
                        prices.publish_time,
                        prices.exponent,
                    ))],
                ),
            ],
        },
    )?;

    Ok(())
}
