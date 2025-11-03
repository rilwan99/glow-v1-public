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
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{Approver, ErrorCode, MarginAccount, PositionConfigUpdate, TokenConfig};

#[derive(Accounts)]
pub struct RegisterPosition<'info> {
    /// The authority that can change the margin account
    pub authority: Signer<'info>,

    /// The address paying for rent
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The margin account to register position type with
    #[account(mut,
        constraint = margin_account.load()?.airspace == config.airspace)]
    pub margin_account: AccountLoader<'info, MarginAccount>,

    /// The mint for the position token being registered
    pub position_token_mint: InterfaceAccount<'info, Mint>,

    /// The margin config for the token
    #[account(constraint = config.mint == position_token_mint.key())]
    pub config: Account<'info, TokenConfig>,

    /// The token account to store hold the position assets in the custody of the
    /// margin account.
    #[account(init,
              seeds = [
                  margin_account.key().as_ref(),
                  position_token_mint.key().as_ref()
              ],
              bump,
              payer = payer,
              token::mint = position_token_mint,
              token::token_program = token_program,
              token::authority = margin_account
    )]
    pub token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub rent: Sysvar<'info, Rent>,
    pub system_program: Program<'info, System>,
}

/// Register a deposit position that uses a PDA that is owned by the margin account.
/// This is used for tokens that are minted by adapters, and thus only [TokenAdmin::Adapter]
/// can use this instruction.
/// If an adapter uses associated token accounts (ATAs) instead of arbitrary PDAs, then
/// the position should be created with `create_deposit_position` which uses ATAs.
pub fn register_position_handler(ctx: Context<RegisterPosition>) -> Result<()> {
    // Only positions of:
    // * TokenKind::Collateral +
    // * TokenAdmin::Adapter
    // can be registered with this instruction.
    // This is the case where a valid adapter has minted collateral tokens to a margin account,
    // and they are being treated as collateral for the margin account.
    //
    // Claims should only be registered by the adapter via CPI as the adapter has to own the claim.
    // AdapterCollateral + TokenAdmin::Adapter can only be registered by CPI, similar to claims.
    // Use `create_deposit_position` for positions that should be owned by the margin account.

    let config = &ctx.accounts.config;
    let adapter = match config.adapter_program() {
        Some(adapter) => adapter,
        None => {
            msg!("register_position only supports TokenAdmin::Adapter");
            return err!(ErrorCode::InvalidConfigRegisterPosition);
        }
    };
    if config.token_kind != crate::TokenKind::Collateral {
        msg!("register_position only supports TokenKind::Collateral");
        return err!(ErrorCode::InvalidConfigRegisterPosition);
    }

    let account = &mut ctx.accounts.margin_account.load_mut()?;
    let position_token = &ctx.accounts.position_token_mint;
    let address = ctx.accounts.token_account.key();
    account.verify_authority(ctx.accounts.authority.key())?;

    if account.has_position(&position_token.key()) {
        return err!(ErrorCode::PositionAlreadyExists);
    }

    account.register_position(
        PositionConfigUpdate::new_from_config(config, position_token.decimals, address, adapter)?,
        &[Approver::MarginAccountAuthority],
    )?;

    Ok(())
}
