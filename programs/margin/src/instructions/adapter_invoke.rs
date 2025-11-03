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

use crate::adapter::{self, IxData};
use crate::syscall::{sys, Sys};
use crate::{events, ErrorCode, MarginAccount};

#[derive(Accounts)]
pub struct AdapterInvoke<'info> {
    /// The authority that owns the margin account
    pub owner: Signer<'info>,

    /// The margin account to proxy an action for
    #[account(mut, has_one = owner)]
    pub margin_account: AccountLoader<'info, MarginAccount>,
}

pub fn adapter_invoke_handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, AdapterInvoke<'info>>,
    instructions: Vec<IxData>,
) -> Result<()> {
    if ctx.accounts.margin_account.load()?.liquidator != Pubkey::default() {
        msg!("account is being liquidated");
        return Err(ErrorCode::Liquidating.into());
    }

    emit!(events::AdapterInvokeBegin {
        margin_account: ctx.accounts.margin_account.key(),
    });

    adapter::invoke_many(
        &ctx.accounts.margin_account,
        ctx.remaining_accounts,
        instructions,
        true,
    )?;

    emit!(events::AdapterInvokeEnd {});

    let margin_account = &mut ctx.accounts.margin_account.load_mut()?;

    margin_account
        .valuation(sys().unix_timestamp())?
        .verify_healthy()?;

    margin_account.assert_position_feature_violation()?;

    Ok(())
}
