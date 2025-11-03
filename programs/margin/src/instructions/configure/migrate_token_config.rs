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

use glow_airspace::state::Airspace;

use crate::migrate::TokenConfig as OldTokenConfig;
use crate::{seeds::TOKEN_CONFIG_SEED, TokenConfig as NewTokenConfig, TOKEN_CONFIG_VERSION};

#[derive(Accounts)]
pub struct MigrateTokenConfig<'info> {
    /// The authority allowed to make changes to configuration
    pub authority: Signer<'info>,

    /// The airspace being modified
    #[account(has_one = authority)]
    pub airspace: Account<'info, Airspace>,

    /// The payer for any rent costs, if required
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The mint for the token being configured
    pub mint: AccountInfo<'info>,

    /// The config account to be modified
    #[account(
        mut,
        seeds = [
            TOKEN_CONFIG_SEED,
            airspace.key().as_ref(),
            mint.key().as_ref()
        ],
        bump,
    )]
    pub token_config: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

pub fn migrate_token_config_handler(ctx: Context<MigrateTokenConfig>) -> Result<()> {
    // Serialize into current version
    require!(
        ctx.accounts.token_config.owner == &crate::ID,
        anchor_lang::error::ErrorCode::ConstraintOwner
    );
    let config = {
        let data = ctx.accounts.token_config.data.borrow();
        OldTokenConfig::try_deserialize(&mut &data[..])?
    };

    let new_config = NewTokenConfig {
        mint: config.mint,
        mint_token_program: config.mint_token_program,
        underlying_mint: config.underlying_mint,
        underlying_mint_token_program: config.underlying_mint_token_program,
        airspace: config.airspace,
        token_kind: config.token_kind,
        value_modifier: config.value_modifier,
        max_staleness: config.max_staleness,
        admin: config.admin,
        token_features: Default::default(),
        version: TOKEN_CONFIG_VERSION,
        reserved: [0; 64],
    };

    // Reallocate the account to the new size
    let existing_balance = ctx.accounts.token_config.lamports();
    let existing_size = ctx.accounts.token_config.data_len();
    let new_size = 8 + std::mem::size_of::<NewTokenConfig>();
    let rent = Rent::get()?;

    let required_rent = rent.minimum_balance(new_size);

    if existing_balance < required_rent {
        let shortfall = required_rent.saturating_sub(existing_balance);
        msg!("Transferring shortfall of {} to config", shortfall);
        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.payer.to_account_info(),
                    to: ctx.accounts.token_config.to_account_info(),
                },
            ),
            shortfall,
        )?;
    }

    msg!("Reallocating config from {} to {}", existing_size, new_size);
    ctx.accounts.token_config.realloc(new_size, true)?;

    // Save the new token config
    let mut config_data = ctx.accounts.token_config.try_borrow_mut_data()?;
    new_config.serialize(&mut &mut config_data[8..])?;

    Ok(())
}
