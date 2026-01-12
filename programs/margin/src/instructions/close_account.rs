/// Instruction context and handler for closing a margin account in the protocol.
///
/// # Overview
/// This module defines the `CloseAccount` context and the `close_account_handler` function,
/// which together allow a user to close their margin account, provided certain conditions are met.
/// When the account is closed, any remaining lamports (rent) are sent to a specified receiver,
/// and an `AccountClosed` event is emitted.
///
/// # Accounts
/// - `owner`: The signer who owns the margin account to be closed. Only the owner can initiate the close.
/// - `receiver`: The account that will receive any remaining lamports from the closed margin account.
/// - `margin_account`: The margin account to be closed. Must be mutable, owned by `owner`, and will be closed
///   (with lamports sent to `receiver`) if all constraints are satisfied.
///
/// # Constraints
/// - The margin account must have no open positions (`positions().count() == 0`).
/// - The margin account must have no outstanding constraints (`constraints.is_empty()`).
///
/// # Behavior
/// - If the account has open positions, the close operation fails with `AccountNotEmpty`.
/// - If the account has outstanding constraints, the close operation fails with `AccountConstraintViolation`.
/// - On successful closure, an `AccountClosed` event is emitted for off-chain tracking and analytics.
///
/// # Security
/// - Ownership is enforced via the `has_one = owner` constraint.
/// - The `close = receiver` attribute ensures that the lamports are safely transferred to the intended recipient.
///
/// # Usage
/// This instruction should be used when a user wishes to fully close their margin account and reclaim any rent.
/// It is typically called after all positions have been settled and no further protocol constraints remain.
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

use crate::{events, ErrorCode, MarginAccount};

#[derive(Accounts)]
pub struct CloseAccount<'info> {
    /// The owner of the account being closed
    pub owner: Signer<'info>,

    /// The account to get any returned rent
    /// CHECK:
    #[account(mut)]
    pub receiver: AccountInfo<'info>,

    /// The account being closed
    // The margin account must:
    // - be mutable (can be closed/modified)
    // - send remaining lamports to `receiver` when closed
    // - have `owner` as its owner field (ownership check)
    #[account(mut,
              close = receiver,
              has_one = owner)]
    pub margin_account: AccountLoader<'info, MarginAccount>,
}

pub fn close_account_handler(ctx: Context<CloseAccount>) -> Result<()> {
    let account = ctx.accounts.margin_account.load()?;

    // Account cannot be closed if user has open position
    if account.positions().count() > 0 {
        return Err(ErrorCode::AccountNotEmpty.into());
    }

    // The `constraints` field is a vector or collection on the MarginAccount struct
    // that tracks any protocol-imposed restrictions (e.g., unsettled borrows, pending obligations).
    // If this is not empty, the account cannot be closed.
    if !account.constraints.is_empty() {
        return Err(ErrorCode::AccountConstraintViolation.into());
    }

    emit!(events::AccountClosed {
        margin_account: ctx.accounts.margin_account.key(),
    });

    Ok(())
}
