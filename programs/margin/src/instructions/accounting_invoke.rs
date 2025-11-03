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
use crate::{events, MarginAccount};

#[derive(Accounts)]
pub struct AccountingInvoke<'info> {
    /// The margin account to proxy an action for
    #[account(mut)]
    pub margin_account: AccountLoader<'info, MarginAccount>,
    // Other accounts are passed as remaining_accounts
}

pub fn accounting_invoke_handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, AccountingInvoke<'info>>,
    instructions: Vec<IxData>,
) -> Result<()> {
    emit!(events::AccountingInvokeBegin {
        margin_account: ctx.accounts.margin_account.key(),
    });

    adapter::invoke_many(
        &ctx.accounts.margin_account,
        ctx.remaining_accounts,
        instructions,
        false,
    )?;

    emit!(events::AccountingInvokeEnd {});

    Ok(())
}
