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
use anchor_spl::token;
use solana_program::clock::UnixTimestamp;

use crate::{
    syscall::{sys, Sys},
    ErrorCode, MarginAccount, PriceChangeInfo, TokenConfig,
};

#[derive(Accounts)]
pub struct RefreshDepositPosition<'info> {
    /// The account to update
    #[account(mut)]
    pub margin_account: AccountLoader<'info, MarginAccount>,

    /// The margin config for the token
    #[account(constraint = config.airspace == margin_account.load()?.airspace @ ErrorCode::WrongAirspace)]
    pub config: Account<'info, TokenConfig>,

    /// The oracle for the token. If the oracle is a redemption rate, it should be the redemption oracle.
    /// If the oracle is not a redemption rate, it should be the price oracle.
    /// CHECK: We verify this account against the pyth pull receiver program
    pub price_oracle: AccountInfo<'info>,

    /// An optional oracle price account for the quote token, if the position uses a redemption rate.
    /// CHECK: We verify this account against the pyth pull receiver program
    pub redemption_quote_oracle: Option<AccountInfo<'info>>,
    // Optional account (remaining accounts)
    // pub position_token_account: XAccount<'info, TokenAccount>,
}

pub fn refresh_deposit_position_handler(ctx: Context<RefreshDepositPosition>) -> Result<()> {
    let margin_account = &mut ctx.accounts.margin_account.load_mut()?;
    let config = &ctx.accounts.config;
    let token_oracle = config.oracle().ok_or(ErrorCode::InvalidOracle)?;

    let clock = Clock::get()?;
    let price_info = PriceChangeInfo::try_from_oracle_accounts(
        &ctx.accounts.price_oracle,
        &ctx.accounts.redemption_quote_oracle,
        &token_oracle,
        &clock,
    )?;

    if let Some(position_token_account) = ctx.remaining_accounts.first() {
        let balance = token::accessor::amount(position_token_account)?;

        margin_account.set_position_balance(
            &config.mint,
            &position_token_account.key(),
            balance,
            sys().unix_timestamp(),
        )?;
    }

    margin_account.set_position_price(
        &config.mint,
        &price_info.to_price_info(sys().unix_timestamp() as UnixTimestamp),
    )?;

    Ok(())
}
