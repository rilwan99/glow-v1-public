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
use bitflags::Flags;
use glow_airspace::state::AirspacePermit;

use crate::{events, AccountFeatureFlags, MarginAccount};

#[derive(Accounts)]
#[instruction(seed: u16)]
pub struct CreateAccount<'info> {
    /// The owner of the new margin account
    pub owner: Signer<'info>,

    /// A permission given to a user address that enables them to use resources within an airspace.
    #[account(has_one = owner)]
    pub permit: Account<'info, AirspacePermit>,

    #[account(mut)]
    pub payer: Signer<'info>,

    /// The margin account to initialize for the owner
    #[account(init,
              seeds = [owner.key.as_ref(), permit.airspace.as_ref(), seed.to_le_bytes().as_ref()],
              bump,
              payer = payer,
              space = 8 + std::mem::size_of::<MarginAccount>(),
    )]
    pub margin_account: AccountLoader<'info, MarginAccount>,

    pub system_program: Program<'info, System>,
}

pub fn create_account_handler(
    ctx: Context<CreateAccount>,
    seed: u16,
    feature_flags: AccountFeatureFlags,
) -> Result<()> {
    // Only one restriction can be set, and it should not be VIOLATION
    require!(
        !feature_flags.contains_unknown_bits(),
        crate::ErrorCode::UnknownFeatureFlags
    );
    require!(
        !feature_flags.contains(AccountFeatureFlags::VIOLATION),
        crate::ErrorCode::InvalidFeatureFlags
    );
    // The set bits should be <= 1
    require!(
        feature_flags.bits().count_ones() <= 1,
        crate::ErrorCode::InvalidFeatureFlags
    );

    let mut account = ctx.accounts.margin_account.load_init()?;

    account.initialize(
        ctx.accounts.permit.airspace,
        *ctx.accounts.owner.key,
        seed,
        ctx.bumps.margin_account,
        feature_flags,
    );

    emit!(events::AccountCreated {
        margin_account: ctx.accounts.margin_account.key(),
        owner: ctx.accounts.owner.key(),
        airspace: ctx.accounts.permit.airspace,
        seed,
        feature_flags,
    });

    Ok(())
}
