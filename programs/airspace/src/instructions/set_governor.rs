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

use crate::events::{GovernorAuthorityTransferCompleted, GovernorAuthorityTransferRequest};
use crate::seeds::GOVERNOR_ID;
use crate::state::{AuthorityTransfer, GovernorId};

#[derive(Accounts)]
#[instruction(proposed_governor: Pubkey)]
pub struct GovernorPropose<'info> {
    /// The current governor
    #[account(mut)]
    governor: Signer<'info>,

    /// The governor identity account
    #[account(mut, has_one = governor)]
    governor_id: Account<'info, GovernorId>,

    /// The choice of seeds ensures that there can only ever be 1 transfer pending
    #[account(
        init,
        seeds = [
            GOVERNOR_ID,
            governor_id.key().as_ref(),
        ],
        bump,
        space = 8 + std::mem::size_of::<AuthorityTransfer>(),
        payer = governor,
    )]
    transfer: Account<'info, AuthorityTransfer>,

    system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct GovernorFinalizePropose<'info> {
    /// The proposed new authority which has to sign to confirm the handover
    proposed_governor: Signer<'info>,

    /// The old governor will receive the rent back
    #[account(mut)]
    old_governor: AccountInfo<'info>,

    /// The airspace to have its authority changed
    #[account(mut, constraint = governor_id.governor == old_governor.key())]
    governor_id: Account<'info, GovernorId>,

    #[account(
        mut,
        close = old_governor,
        constraint = transfer.resource == governor_id.key(),
        constraint = transfer.current_authority == old_governor.key(),
        constraint = transfer.new_authority == proposed_governor.key(),
    )]
    transfer: Account<'info, AuthorityTransfer>,
}

pub fn governor_propose(ctx: Context<GovernorPropose>, proposed_governor: Pubkey) -> Result<()> {
    let transfer = &mut ctx.accounts.transfer;
    transfer.resource = ctx.accounts.governor_id.key();
    transfer.current_authority = ctx.accounts.governor.key();
    transfer.new_authority = proposed_governor;

    emit!(GovernorAuthorityTransferRequest {
        governor_id: ctx.accounts.governor_id.key(),
        current_governor: ctx.accounts.governor.key(),
        proposed_governor,
    });

    Ok(())
}

pub fn governor_finalize_propose(ctx: Context<GovernorFinalizePropose>) -> Result<()> {
    let transfer = &ctx.accounts.transfer;
    let g = &mut ctx.accounts.governor_id;
    g.governor = transfer.new_authority;

    emit!(GovernorAuthorityTransferCompleted {
        governor_id: transfer.resource,
        new_governor: g.governor,
    });

    Ok(())
}
