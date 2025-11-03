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
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{Approver, ErrorCode, MarginAccount, PositionConfigUpdate, TokenConfig};

#[derive(Accounts)]
pub struct CreateDepositPosition<'info> {
    /// The authority that can change the margin account
    pub authority: Signer<'info>,

    /// The address paying for rent
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The margin account to register this deposit account with
    #[account(mut)]
    pub margin_account: AccountLoader<'info, MarginAccount>,

    /// The mint for the token being stored in this account
    pub mint: InterfaceAccount<'info, Mint>,

    /// The margin config for the token
    #[account(
        has_one = mint,
        constraint = config.airspace == margin_account.load()?.airspace @ ErrorCode::WrongAirspace
    )]
    pub config: Account<'info, TokenConfig>,

    /// The token account to store deposits
    #[account(
        associated_token::mint = mint,
        associated_token::authority = margin_account,
        associated_token::token_program = mint_token_program
    )]
    pub token_account: InterfaceAccount<'info, TokenAccount>,

    pub associated_token_program: Program<'info, AssociatedToken>,
    pub mint_token_program: Interface<'info, TokenInterface>,
    pub rent: Sysvar<'info, Rent>,
    pub system_program: Program<'info, System>,
}

/// Create a deposit position and register it as collateral with the margin account.
///
/// The token mint can be an SPL token or an adapter owned token, thus users have an option of creating two
/// types of tokens:
/// - Position ATA, created as an ATA by the caller, and registered with this instruction.
/// - Position PDA, created with seeds [margin_account, mint] using register_position.
///
/// Why would a user create an ATA and register it as collateral? This creates an opt-in system where a user
/// can store different tokens in their margin account, and choose which ones they opt in or out of being
/// treated as collateral.
pub fn create_deposit_position_handler(ctx: Context<CreateDepositPosition>) -> Result<()> {
    // Only TokenKind::Collateral positions can be registered with this instruction.

    let config = &ctx.accounts.config;

    if config.token_kind != crate::TokenKind::Collateral {
        msg!("create_deposit_position only supports TokenKind::Collateral");
        return err!(crate::ErrorCode::InvalidConfigRegisterPosition);
    }

    let account = &mut ctx.accounts.margin_account.load_mut()?;
    let position_token = &ctx.accounts.mint;
    let address = ctx.accounts.token_account.key();
    account.verify_authority(ctx.accounts.authority.key())?;

    account.register_position(
        PositionConfigUpdate::new_from_config(
            config,
            position_token.decimals,
            address,
            config.adapter_program().unwrap_or_default(),
        )?,
        &[Approver::MarginAccountAuthority],
    )?;

    Ok(())
}
