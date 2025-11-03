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

use crate::{
    events::{AirspaceAuthorityCancelTransfer, AirspaceAuthoritySet, AirspaceAuthorityTransfer},
    seeds::AIRSPACE,
    state::{Airspace, AuthorityTransfer},
    AirspaceErrorCode,
};

#[derive(Accounts)]
pub struct AirspaceAuthorityPropose<'info> {
    /// The current airspace authority
    #[account(mut)]
    authority: Signer<'info>,

    /// The airspace to have its authority changed
    #[account(mut, has_one = authority)]
    airspace: Account<'info, Airspace>,

    /// The choice of seeds ensures that there can only ever be 1 transfer pending
    #[account(
        init,
        seeds = [
            AIRSPACE,
            airspace.key().as_ref(),
        ],
        bump,
        space = 8 + std::mem::size_of::<AuthorityTransfer>(),
        payer = authority,
    )]
    transfer: Account<'info, AuthorityTransfer>,

    system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AirspaceAuthorityCancelProposal<'info> {
    #[account(mut)]
    authority: Signer<'info>,

    #[account(mut, has_one = authority)]
    airspace: Account<'info, Airspace>,

    #[account(
        mut,
        close = authority, // Close the proposed authority account
        constraint = transfer.resource == airspace.key(),
        constraint = transfer.current_authority == authority.key(),
    )]
    transfer: Account<'info, AuthorityTransfer>,

    system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AirspaceAuthorityFinalize<'info> {
    /// The proposed new authority which has to sign to confirm the handover
    proposed_authority: Signer<'info>,

    /// The old authority will receive the rent back
    #[account(mut)]
    old_authority: AccountInfo<'info>,

    /// The airspace to have its authority changed
    #[account(
        mut, constraint = airspace.authority == old_authority.key())]
    airspace: Account<'info, Airspace>,

    #[account(
        mut,
        close = old_authority,
        constraint = transfer.resource == airspace.key(),
        constraint = transfer.current_authority == old_authority.key(),
        constraint = transfer.new_authority == proposed_authority.key(),
    )]
    transfer: Account<'info, AuthorityTransfer>,
}

pub fn airspace_transfer_authority_handler(
    ctx: Context<AirspaceAuthorityPropose>,
    proposed_authority: Pubkey,
) -> Result<()> {
    if proposed_authority == Pubkey::default() {
        msg!("A new proposed airspace authority cannot be the zero key");
        return err!(AirspaceErrorCode::PermissionDenied);
    }

    let transfer = &mut ctx.accounts.transfer;
    transfer.resource = ctx.accounts.airspace.key();
    transfer.current_authority = ctx.accounts.authority.key();
    transfer.new_authority = proposed_authority;

    emit!(AirspaceAuthorityTransfer {
        airspace: ctx.accounts.airspace.key(),
        current_authority: ctx.accounts.authority.key(),
        proposed_authority,
    });

    Ok(())
}

pub fn airspace_authority_cancel_proposal(
    ctx: Context<AirspaceAuthorityCancelProposal>,
) -> Result<()> {
    let transfer = &mut ctx.accounts.transfer;
    let proposed_authority = transfer.new_authority;
    transfer.new_authority = Pubkey::default();

    emit!(AirspaceAuthorityCancelTransfer {
        airspace: ctx.accounts.airspace.key(),
        current_authority: ctx.accounts.authority.key(),
        proposed_authority,
    });

    Ok(())
}

pub fn airspace_authority_finalize(ctx: Context<AirspaceAuthorityFinalize>) -> Result<()> {
    let transfer = &ctx.accounts.transfer;
    let airspace = &mut ctx.accounts.airspace;
    airspace.authority = transfer.new_authority;

    emit!(AirspaceAuthoritySet {
        airspace: airspace.key(),
        authority: airspace.authority,
    });

    Ok(())
}
