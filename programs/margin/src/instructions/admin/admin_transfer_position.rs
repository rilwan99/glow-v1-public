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

use anchor_spl::{
    token_2022::TransferChecked,
    token_interface::{self, Mint, TokenAccount, TokenInterface},
};
use glow_program_common::PROTOCOL_GOVERNOR_ID;

use crate::{
    events::TransferPosition,
    syscall::{sys, Sys},
    MarginAccount, SignerSeeds,
};

#[derive(Accounts)]
pub struct AdminTransferPosition<'info> {
    /// The administrative authority
    #[account(address = PROTOCOL_GOVERNOR_ID)]
    pub authority: Signer<'info>,

    /// The target margin account to move a position into
    #[account(mut)]
    pub target_account: AccountLoader<'info, MarginAccount>,

    /// The source account to move a position out of
    #[account(mut)]
    pub source_account: AccountLoader<'info, MarginAccount>,

    /// The token account to be moved from
    #[account(mut, token::mint = token_mint, token::token_program = token_program)]
    pub source_token_account: InterfaceAccount<'info, TokenAccount>,

    /// The token account to be moved into
    #[account(mut, token::mint = token_mint)]
    pub target_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_mint: InterfaceAccount<'info, Mint>,

    pub token_program: Interface<'info, TokenInterface>,
}

impl<'info> AdminTransferPosition<'info> {
    fn transfer_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            TransferChecked {
                from: self.source_token_account.to_account_info(),
                to: self.target_token_account.to_account_info(),
                authority: self.source_account.to_account_info(),
                mint: self.token_mint.to_account_info(),
            },
        )
    }
}

pub fn admin_transfer_position_handler(
    ctx: Context<AdminTransferPosition>,
    amount: u64,
) -> Result<()> {
    let source_seeds = ctx.accounts.source_account.load()?.signer_seeds_owned();

    token_interface::transfer_checked(
        ctx.accounts
            .transfer_context()
            .with_signer(&[&source_seeds.signer_seeds()]),
        amount,
        ctx.accounts.token_mint.decimals,
    )?;

    let source_tokens = &mut ctx.accounts.source_token_account;
    let target_tokens = &mut ctx.accounts.target_token_account;

    source_tokens.reload()?;
    target_tokens.reload()?;

    let source = &mut ctx.accounts.source_account.load_mut()?;
    let target = &mut ctx.accounts.target_account.load_mut()?;

    source.set_position_balance(
        &source_tokens.mint,
        &source_tokens.key(),
        source_tokens.amount,
        sys().unix_timestamp(),
    )?;
    target.set_position_balance(
        &target_tokens.mint,
        &target_tokens.key(),
        target_tokens.amount,
        sys().unix_timestamp(),
    )?;

    emit!(TransferPosition {
        source_margin_account: ctx.accounts.source_account.key(),
        target_margin_account: ctx.accounts.target_account.key(),
        source_token_account: ctx.accounts.source_token_account.key(),
        target_token_account: ctx.accounts.target_token_account.key(),
        amount
    });

    Ok(())
}
