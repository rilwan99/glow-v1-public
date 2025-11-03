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
    token_2022::{self, CloseAccount},
    token_interface::{Mint, TokenAccount, TokenInterface},
};
use glow_margin::{AdapterResult, MarginAccount, PositionChange, TokenConfig};

use crate::{seeds::*, state::TestServiceAuthority};

#[derive(Accounts)]
pub struct CloseAdapterPosition<'info> {
    #[account(mut)]
    owner: Signer<'info>,

    airspace: AccountInfo<'info>,

    #[account(
        seeds = [
            TEST_SERVICE_AUTHORITY,
        ],
        bump
    )]
    test_authority: Box<Account<'info, TestServiceAuthority>>,

    #[account(
        mut,
        has_one = airspace,
        has_one = owner,
    )]
    margin_account: AccountLoader<'info, MarginAccount>,

    #[account(
        has_one = airspace,
        constraint = token_config.adapter_program() == Some(crate::ID),
        constraint = token_config.mint == position_mint.key(),
    )]
    token_config: Box<Account<'info, TokenConfig>>,

    position_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        seeds = [
            TOKEN_ACCOUNT,
            position_mint.key().as_ref()
        ],
        bump,
    )]
    position_account: InterfaceAccount<'info, TokenAccount>,

    token_program: Interface<'info, TokenInterface>,
    system_program: Program<'info, System>,
}

impl<'info> CloseAdapterPosition<'info> {
    fn close_token_account_ctx(&self) -> CpiContext<'_, '_, '_, 'info, CloseAccount<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            CloseAccount {
                account: self.position_account.to_account_info(),
                authority: self.test_authority.to_account_info(),
                destination: self.owner.to_account_info(),
            },
        )
    }
}

/// [2025-08 Audit remediations]
/// This instruction exists solely to test registering AdapterCollateral positions as the margin
/// program currently does not have any adapter that can register collateral positions.
pub fn close_adapter_position_handler(ctx: Context<CloseAdapterPosition>) -> Result<()> {
    token_2022::close_account(
        ctx.accounts
            .close_token_account_ctx()
            .with_signer(&[&ctx.accounts.test_authority.signer_seeds()]),
    )?;

    glow_margin::write_adapter_result(
        &*ctx.accounts.margin_account.load()?,
        &AdapterResult {
            position_changes: vec![(
                ctx.accounts.position_mint.key(),
                vec![PositionChange::Close(ctx.accounts.position_account.key())],
            )],
        },
    )?;

    Ok(())
}
