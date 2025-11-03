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

use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface};
use glow_program_common::{traits::SafeSub, Number128};

use crate::{
    syscall::{sys, Sys},
    LiquidationState, MarginAccount, PriceChangeInfo, SignerSeeds, TokenAdmin, TokenConfig,
};

#[derive(Accounts)]
pub struct CollectLiquidationFee<'info> {
    /// The account in need of liquidation
    #[account(mut)]
    pub margin_account: AccountLoader<'info, MarginAccount>,

    /// The liquidator account performing the liquidation actions
    #[account(mut)]
    pub liquidator: Signer<'info>,

    /// Account to persist the state of the liquidation
    #[account(
        mut,
        seeds = [
            b"liquidation",
            margin_account.key().as_ref(),
            liquidator.key().as_ref()
        ],
        bump,
    )]
    pub liquidation: AccountLoader<'info, LiquidationState>,

    /// The liquidator's token account
    #[account(
        mut,
        token::mint = liquidation_fee_mint,
        token::authority = liquidator,
        token::token_program = liquidator_fee_token_program,
    )]
    pub liquidator_fee_token: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = liquidation_fee_mint,
        token::authority = margin_account,
        token::token_program = liquidator_fee_token_program,
    )]
    pub margin_account_fee_source: Box<InterfaceAccount<'info, TokenAccount>>,

    pub liquidation_fee_mint: Box<InterfaceAccount<'info, Mint>>,

    pub liquidator_fee_token_program: Interface<'info, TokenInterface>,

    #[account(
        constraint = token_config.airspace == margin_account.load()?.airspace,
        constraint = token_config.mint == liquidation_fee_mint.key(),
    )]
    pub token_config: Box<Account<'info, TokenConfig>>,

    /// The oracle for the token. If the oracle is a redemption rate, it should be the redemption oracle.
    /// If the oracle is not a redemption rate, it should be the price oracle.
    /// CHECK: We verify this account against the pyth pull receiver program
    pub price_oracle: AccountInfo<'info>,

    /// An optional oracle price account for the quote token, if the position uses a redemption rate.
    /// CHECK: We verify this account against the pyth pull receiver program
    pub redemption_quote_oracle: Option<AccountInfo<'info>>,
}

pub fn collect_liquidation_fee_handler(ctx: Context<CollectLiquidationFee>) -> Result<()> {
    // SECURITY: Oracle ownership is validated with [verify_oracle_ownership] when constructing the price.

    let margin_account = &ctx.accounts.margin_account;

    let fee_mint = ctx.accounts.liquidation_fee_mint.key();
    let liquidation = &mut ctx.accounts.liquidation.load_mut()?.state;
    // Update liquidation state
    liquidation.is_collecting_fees = 1;
    let liquidation_slot = liquidation
        .accrued_liquidation_fees
        .iter()
        .find(|p| p.mint == fee_mint)
        .ok_or(crate::ErrorCode::InvalidLiquidationFeeMint)?;

    let timestamp = sys().unix_timestamp();

    // Need oracle to validate fee
    let TokenAdmin::Margin { oracle } = ctx.accounts.token_config.admin else {
        return err!(crate::ErrorCode::InvalidOracle);
    };

    let clock = Clock::get()?;
    let price_info = PriceChangeInfo::try_from_oracle_accounts(
        &ctx.accounts.price_oracle,
        &ctx.accounts.redemption_quote_oracle,
        &oracle,
        &clock,
    )?
    .to_price_info(clock.unix_timestamp);

    let decimals: i32 = -(ctx.accounts.liquidation_fee_mint.decimals as i32);

    // Get the price if it's valid
    let price_as_num128 = price_info.to_number128()?;
    // The fee is offset against the equity loss incurred.
    let mut liquidation_fee =
        Number128::from_decimal(liquidation_slot.amount, decimals) * price_as_num128;
    // If value lost > liquidation fee, absorb all the fee
    if liquidation.equity_loss() <= &Number128::ZERO {
        // Congratulate the liquidator for making the user better, they can take their whole fee
    } else if liquidation.equity_loss() > &liquidation_fee {
        *liquidation.equity_loss_mut() = liquidation.equity_loss().safe_sub(liquidation_fee)?;
        liquidation_fee = Number128::ZERO;
    } else {
        liquidation_fee = liquidation_fee.safe_sub(*liquidation.equity_loss())?;
        *liquidation.equity_loss_mut() = Number128::ZERO;
    }

    // Convert liquidation fee back to tokens, and take the tokens
    let fee_tokens = (liquidation_fee / price_as_num128).as_u64(decimals);

    if fee_tokens > 0 {
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.liquidator_fee_token_program.to_account_info(),
                token_interface::TransferChecked {
                    from: ctx.accounts.margin_account_fee_source.to_account_info(),
                    mint: ctx.accounts.liquidation_fee_mint.to_account_info(),
                    to: ctx.accounts.liquidator_fee_token.to_account_info(),
                    authority: ctx.accounts.margin_account.to_account_info(),
                },
            )
            .with_signer(&[&margin_account.load()?.signer_seeds()]),
            fee_tokens,
            ctx.accounts.liquidation_fee_mint.decimals,
        )?;

        ctx.accounts.margin_account_fee_source.reload()?;
        let token_account = &ctx.accounts.margin_account_fee_source;
        let balance = anchor_spl::token::accessor::amount(&token_account.to_account_info())?;

        // Update the margin account after taking fee
        margin_account.load_mut()?.set_position_balance(
            &token_account.mint,
            &token_account.key(),
            balance,
            timestamp,
        )?;
    }

    // Reset the fee slot
    liquidation.clear_liquidation_fee(fee_mint);

    margin_account
        .load()?
        .valuation(timestamp)?
        .verify_healthy()?;

    Ok(())
}
