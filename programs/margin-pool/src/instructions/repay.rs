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
use anchor_spl::token_interface::{
    self, Burn, Mint, TokenAccount, TokenInterface, TransferChecked,
};
use glow_margin::{AdapterResult, PositionChange, TokenBalanceChange, TokenBalanceChangeCause};
use glow_program_common::token_change::{ChangeKind, TokenChange};

use crate::{events, state::PoolAction, Amount, ErrorCode, MarginPool};

#[derive(Accounts)]
pub struct Repay<'info> {
    /// The pool with the outstanding loan
    #[account(
        mut,
        has_one = loan_note_mint,
        has_one = vault
    )]
    pub margin_pool: Box<Account<'info, MarginPool>>,

    /// The mint for the notes representing loans from the pool
    /// CHECK:
    #[account(mut)]
    pub loan_note_mint: AccountInfo<'info>,

    /// CHECK: Checked by Token program, we can't transfer incorrect token to our vault, which is checked
    pub token_mint: InterfaceAccount<'info, Mint>,

    /// The vault responsible for storing the pool's tokens
    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    /// The account with the loan notes
    #[account(mut)]
    pub loan_account: InterfaceAccount<'info, TokenAccount>,

    /// The token account repaying the debt
    #[account(mut)]
    pub repayment_token_account: InterfaceAccount<'info, TokenAccount>,

    /// Signing authority for the repaying token account
    pub repayment_account_authority: Signer<'info>,

    pub mint_token_program: Interface<'info, TokenInterface>,
    pub pool_token_program: Interface<'info, TokenInterface>,
}

impl<'info> Repay<'info> {
    fn burn_loan_context(&self) -> CpiContext<'_, '_, '_, 'info, Burn<'info>> {
        CpiContext::new(
            self.pool_token_program.to_account_info(),
            Burn {
                mint: self.loan_note_mint.to_account_info(),
                from: self.loan_account.to_account_info(),
                authority: self.margin_pool.to_account_info(),
            },
        )
    }

    fn transfer_repayment_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        CpiContext::new(
            self.mint_token_program.to_account_info(),
            TransferChecked {
                from: self.repayment_token_account.to_account_info(),
                to: self.vault.to_account_info(),
                authority: self.repayment_account_authority.to_account_info(),
                mint: self.token_mint.to_account_info(),
            },
        )
    }
}

pub fn repay_handler(ctx: Context<Repay>, change_kind: ChangeKind, amount: u64) -> Result<()> {
    let change = TokenChange {
        kind: change_kind,
        tokens: amount,
    };
    let pool = &mut ctx.accounts.margin_pool;
    let clock = Clock::get()?;

    // Make sure interest accrual is up-to-date
    if !pool.accrue_interest(clock.unix_timestamp) {
        msg!("interest accrual is too far behind");
        return Err(ErrorCode::InterestAccrualBehind.into());
    }

    // Amount the user desires to repay
    let source_balance = pool.convert_amount(
        Amount::tokens(ctx.accounts.repayment_token_account.amount),
        PoolAction::Borrow,
    )?;
    let destination_balance = pool.convert_amount(
        Amount::notes(ctx.accounts.loan_account.amount),
        PoolAction::Deposit,
    )?;
    let repay_amount = pool.calculate_full_amount(
        source_balance,
        destination_balance,
        change,
        PoolAction::Repay,
    )?;

    pool.repay(&repay_amount)?;

    // Finish by transferring the requisite tokens and burning the loan notes
    let pool = &ctx.accounts.margin_pool;
    let signer = [&pool.signer_seeds()?[..]];

    token_interface::transfer_checked(
        ctx.accounts.transfer_repayment_context(),
        repay_amount.tokens,
        ctx.accounts.token_mint.decimals,
    )?;
    token_interface::burn(
        ctx.accounts.burn_loan_context().with_signer(&signer),
        repay_amount.notes,
    )?;

    emit!(events::Repay {
        margin_pool: pool.key(),
        user: ctx.accounts.repayment_account_authority.key(),
        loan_account: ctx.accounts.loan_account.key(),
        repayment_token_account: ctx.accounts.repayment_token_account.key(),
        repaid_tokens: repay_amount.tokens,
        repaid_loan_notes: repay_amount.notes,
        summary: (&pool.clone().into_inner()).into(),
    });

    // Tell the margin program how much has been repaid (if relevant), and what current prices are
    let mut adapter_result_data = vec![];
    let adapter_result = AdapterResult {
        position_changes: vec![(
            pool.token_mint,
            vec![PositionChange::TokenChange(TokenBalanceChange {
                mint: pool.token_mint,
                tokens: repay_amount.tokens,
                change_cause: TokenBalanceChangeCause::Repay,
            })],
        )],
    };
    adapter_result.serialize(&mut adapter_result_data)?;
    anchor_lang::solana_program::program::set_return_data(&adapter_result_data);
    Ok(())
}
