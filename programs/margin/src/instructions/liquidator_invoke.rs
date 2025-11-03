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
use anchor_spl::token_interface::{Mint, TokenInterface};
use glow_program_common::Number128;

use crate::adapter::{self, IxData};
use crate::syscall::{sys, Sys};
use crate::{
    events, ErrorCode, Liquidation, LiquidationState, MarginAccount, TokenBalanceChangeCause,
    Valuation, LIQUIDATION_FEE_BPS,
};

#[derive(Accounts)]
pub struct LiquidatorInvoke<'info> {
    /// The liquidator processing the margin account
    pub liquidator: Signer<'info>,

    /// Account to persist the state of the liquidation
    #[account(mut,
        has_one = liquidator,
        has_one = margin_account,
        constraint = liquidation.load()?.state.is_collecting_fees == 0,
    )]
    pub liquidation: AccountLoader<'info, LiquidationState>,

    /// The margin account to proxy an action for
    #[account(mut)]
    pub margin_account: AccountLoader<'info, MarginAccount>,

    /// The mint in which the liquidator will accrue its fee.
    /// This is normally the destination token, e.g. when repaying a loan.
    pub liquidator_fee_mint: Box<InterfaceAccount<'info, Mint>>,

    pub liquidator_fee_token_program: Interface<'info, TokenInterface>,
}

pub fn liquidator_invoke_handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, LiquidatorInvoke<'info>>,
    instructions: Vec<IxData>,
) -> Result<()> {
    let margin_account = &ctx.accounts.margin_account;
    // let adapter_program: &AccountInfo<'info> = &ctx.accounts.adapter_program;
    let remaining_accounts: &'c [AccountInfo<'info>] = ctx.remaining_accounts;
    let start_value = margin_account.load()?.valuation(sys().unix_timestamp())?;

    emit!(events::LiquidatorInvokeBegin {
        margin_account: ctx.accounts.margin_account.key(),
        liquidator: ctx.accounts.liquidator.key(),
    });

    let token_changes =
        adapter::invoke_many(margin_account, remaining_accounts, instructions, true)?;

    // Determine the token changes of the relevant token to take a fee in.
    let fee_relevant_changes = token_changes
        .iter()
        .filter(|c| {
            // Relevant changes are only the changes in the tokens of the fee mint,
            // and the changes that a liquidator is expected to make while liquidating.
            c.mint == ctx.accounts.liquidator_fee_mint.key()
                && [
                    TokenBalanceChangeCause::Borrow,
                    TokenBalanceChangeCause::Repay,
                    TokenBalanceChangeCause::ExternalDecrease,
                    TokenBalanceChangeCause::ExternalIncrease,
                ]
                .contains(&c.change_cause)
        })
        .collect::<Vec<_>>();
    // The fee for swaps is based on the lower of the increase in the token and the repaid amount
    let increases: i128 = fee_relevant_changes
        .iter()
        .map(|c| {
            match c.change_cause {
                TokenBalanceChangeCause::ExternalIncrease => c.tokens as i128,
                // Offset increases
                TokenBalanceChangeCause::ExternalDecrease => -(c.tokens as i128),
                _ => 0,
            }
        })
        .sum();

    let repayments: i128 = fee_relevant_changes
        .iter()
        .map(|c| match c.change_cause {
            TokenBalanceChangeCause::Borrow => -(c.tokens as i128),
            TokenBalanceChangeCause::Repay => c.tokens as i128,
            _ => 0,
        })
        .sum();

    if repayments < 0 {
        msg!("Liquidator has a net borrow of {}", repayments);
        return err!(crate::ErrorCode::LiquidationLostValue);
    }

    let fee_eligible_tokens: u64 = increases
        .min(repayments)
        // max 0 to remove any negative values
        .max(0)
        .try_into()
        .map_err(|_| crate::ErrorCode::MathOpFailed)?;

    // Accrue a liquidator fee if any.
    // The liquidation fee is calculated as:
    //  * x/(100 + x) of the eligible amount
    //  * minus any value lost during liquidation (e.g. slippage from swapping)
    let decimals = ctx.accounts.liquidator_fee_mint.decimals;
    let liquidation_fee = Number128::from_bps(LIQUIDATION_FEE_BPS)
        / (Number128::ONE + Number128::from_bps(LIQUIDATION_FEE_BPS))
        * Number128::from_decimal(fee_eligible_tokens, decimals);
    let liquidation_fee = liquidation_fee.as_u64(decimals);

    {
        let liquidation = &mut ctx.accounts.liquidation.load_mut()?.state;
        liquidation
            .accrue_liquidation_fee(ctx.accounts.liquidator_fee_mint.key(), liquidation_fee)?;
    }

    let liquidation = &mut ctx.accounts.liquidation.load_mut()?.state;
    let end_value = update_and_verify_liquidation(
        &*ctx.accounts.margin_account.load()?,
        liquidation,
        start_value,
    )?;

    emit!(events::LiquidatorInvokeEnd {
        liquidation_data: *liquidation,
        valuation_summary: end_value.into(),
        accrued_liquidation_fee_amount: liquidation_fee,
        liquidation_fee_mint: ctx.accounts.liquidator_fee_mint.key(),
    });

    Ok(())
}

fn update_and_verify_liquidation(
    margin_account: &MarginAccount,
    liquidation: &mut Liquidation,
    start_value: Valuation,
) -> Result<Valuation> {
    let end_value = margin_account.valuation(sys().unix_timestamp())?;

    *liquidation.equity_loss_mut() += start_value.equity - end_value.equity;
    *liquidation.collateral_change_mut() +=
        end_value.available_collateral() - start_value.available_collateral();

    if liquidation.equity_loss() > &liquidation.max_equity_loss() {
        msg!(
            "Illegal liquidation: net loss of {} equity which exceeds the max equity loss of {}",
            liquidation.equity_loss(),
            liquidation.max_equity_loss()
        );
        return err!(ErrorCode::LiquidationLostValue);
    }
    let collateral_change = liquidation.collateral_change();
    if collateral_change < &Number128::ZERO {
        // Liquidation shouldn't reduce the available collateral
        msg!(
            "Illegal liquidation: net reduction in available collateral ({}) not allowed.",
            collateral_change
        );
        return err!(ErrorCode::LiquidationLostValue);
    }

    let available_collateral = end_value.available_collateral();
    if &available_collateral > liquidation.max_available_collateral_limit() {
        msg!(
            "Illegal available collateral of {} exceeds the maximum {} allowed",
            available_collateral,
            liquidation.max_available_collateral_limit()
        );
        return err!(ErrorCode::LiquidationLostValue);
    }

    Ok(end_value)
}
