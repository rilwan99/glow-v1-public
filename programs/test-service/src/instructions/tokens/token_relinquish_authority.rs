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
    token_2022::SetAuthority,
    token_interface::{self, Mint, TokenInterface},
};

use crate::{seeds::TOKEN_INFO, state::TokenInfo};

#[derive(Accounts)]
pub struct TokenRelinquishAuthority<'info> {
    /// user paying fees
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The relevant token mint
    #[account(
        mut,
        mint::token_program = token_program,
    )]
    pub mint: InterfaceAccount<'info, Mint>,

    /// The test info for the token
    #[account(has_one = mint)]
    pub info: Account<'info, TokenInfo>,

    pub new_authority: AccountInfo<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn token_relinquish_authority_handler(ctx: Context<TokenRelinquishAuthority>) -> Result<()> {
    let info = &mut ctx.accounts.info;

    // Only allow this for glowSOL
    require!(
        info.symbol.to_lowercase() == "glowsol",
        crate::error::TestServiceError::UnauthorizedAuthorityTransfer
    );

    let bump_seed = info.bump_seed;

    token_interface::set_authority(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            SetAuthority {
                current_authority: ctx.accounts.info.to_account_info(),
                account_or_mint: ctx.accounts.mint.to_account_info(),
            },
            &[&[TOKEN_INFO, ctx.accounts.mint.key().as_ref(), &[bump_seed]]],
        ),
        token_interface::spl_token_2022::instruction::AuthorityType::MintTokens,
        Some(ctx.accounts.new_authority.key()),
    )?;

    if ctx.accounts.mint.freeze_authority.is_some() {
        token_interface::set_authority(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                SetAuthority {
                    current_authority: ctx.accounts.info.to_account_info(),
                    account_or_mint: ctx.accounts.mint.to_account_info(),
                },
                &[&[TOKEN_INFO, ctx.accounts.mint.key().as_ref(), &[bump_seed]]],
            ),
            token_interface::spl_token_2022::instruction::AuthorityType::FreezeAccount,
            Some(ctx.accounts.new_authority.key()),
        )?;
    }

    Ok(())
}
