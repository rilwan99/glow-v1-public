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

use std::ops::Deref;

use anchor_lang::prelude::*;
use anchor_spl::{
    token_2022::{Burn, TransferChecked},
    token_interface::{self, Mint, TokenAccount, TokenInterface},
};

use crate::{events, state::*, Amount};

#[derive(Accounts)]
pub struct WithdrawFees<'info> {
    /// Fee owner
    fee_owner: Signer<'info>,

    /// The pool to withdraw from
    #[account(
              has_one = vault,
              has_one = deposit_note_mint,
              has_one = fee_destination)]
    pub margin_pool: Account<'info, MarginPool>,

    /// The vault for the pool, where tokens are held
    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    /// The account to withdraw the fees from
    #[account(
        mut,
        token::mint = deposit_note_mint,
        token::authority = fee_owner,
        token::token_program = pool_token_program
    )]
    pub fee_destination: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = token_mint,
        token::authority = fee_owner,
        token::token_program = mint_token_program
    )]
    pub fee_withdrawal_destination: InterfaceAccount<'info, TokenAccount>,

    /// The mint for the underlying token
    pub token_mint: InterfaceAccount<'info, Mint>,

    /// The mint for the deposit notes
    #[account(mut)]
    pub deposit_note_mint: InterfaceAccount<'info, Mint>,

    pub mint_token_program: Interface<'info, TokenInterface>,
    pub pool_token_program: Interface<'info, TokenInterface>,
}

impl<'info> WithdrawFees<'info> {
    fn transfer_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        CpiContext::new(
            self.mint_token_program.to_account_info(),
            TransferChecked {
                to: self.fee_withdrawal_destination.to_account_info(),
                from: self.vault.to_account_info(),
                authority: self.margin_pool.to_account_info(),
                mint: self.token_mint.to_account_info(),
            },
        )
    }

    fn burn_note_context(&self) -> CpiContext<'_, '_, '_, 'info, Burn<'info>> {
        CpiContext::new(
            self.pool_token_program.to_account_info(),
            Burn {
                from: self.fee_destination.to_account_info(),
                mint: self.deposit_note_mint.to_account_info(),
                authority: self.fee_owner.to_account_info(),
            },
        )
    }
}

pub fn withdraw_fees_handler(ctx: Context<WithdrawFees>) -> Result<()> {
    let pool = &ctx.accounts.margin_pool;
    let signer = [&pool.signer_seeds()?[..]];

    let fee_notes = ctx.accounts.fee_destination.amount;

    let claimed_amount = pool.convert_amount(Amount::notes(fee_notes), PoolAction::Withdraw)?;

    token_interface::transfer_checked(
        ctx.accounts.transfer_context().with_signer(&signer),
        claimed_amount.tokens,
        ctx.accounts.token_mint.decimals,
    )?;
    token_interface::burn(ctx.accounts.burn_note_context(), claimed_amount.notes)?;

    emit!(events::FeesWithdrawn {
        margin_pool: pool.key(),
        summary: pool.deref().into(),
        fee_owner: ctx.accounts.fee_owner.key(),
        fee_withdrawal_destination: ctx.accounts.fee_withdrawal_destination.key(),
        fee_notes_withdrawn: claimed_amount.notes,
        fee_tokens_withdrawn: claimed_amount.tokens
    });

    Ok(())
}
