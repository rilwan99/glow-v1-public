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
    AccountFeatureFlags, ErrorCode, MarginAccount, Permissions, Permit, TokenConfig, TokenFeatures,
};

#[derive(Accounts)]
pub struct RefreshPositionConfig<'info> {
    /// The margin account with the position to be refreshed
    #[account(mut)]
    pub margin_account: AccountLoader<'info, MarginAccount>,

    /// The config account for the token, which has been updated
    #[account(constraint = config.airspace == margin_account.load()?.airspace @ ErrorCode::WrongAirspace)]
    pub config: Account<'info, TokenConfig>,

    /// permit that authorizes the refresher
    #[account(constraint = permit.airspace == margin_account.load()?.airspace @ ErrorCode::WrongAirspace)]
    pub permit: Account<'info, Permit>,

    /// account that is authorized to refresh position metadata
    pub refresher: Signer<'info>,
}

/// Refresh the metadata for a position
pub fn refresh_position_config_handler(ctx: Context<RefreshPositionConfig>) -> Result<()> {
    let account = &mut ctx.accounts.margin_account.load_mut()?;

    ctx.accounts.permit.validate(
        account.airspace,
        ctx.accounts.refresher.key(),
        Permissions::REFRESH_POSITION_CONFIG,
    )?;
    let config = &ctx.accounts.config;

    account.refresh_position_metadata(
        &config.mint,
        config.token_kind,
        config.value_modifier,
        config.max_staleness,
        config.token_features,
    )?;

    // This is the only instance where a restricted feature could be assigned to a margin account.
    // If a token has become restricted, and the margin account has no features, set it as violating.
    // If the account is already violating, this becomes a no-op, thus it's safe to check for emptiness.
    //
    // To remedy a violation, the user will be required to close the position. This is checked at the
    // end of margin invocation, thus the user should have adequate freedom to perform multiple actions that
    // remedy the violation.
    if account.features.is_empty() && config.token_features.contains(TokenFeatures::RESTRICTED) {
        account.features.set(AccountFeatureFlags::VIOLATION, true);
    }

    Ok(())
}
