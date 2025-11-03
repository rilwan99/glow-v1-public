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
use anchor_spl::{token::ID as TOKEN_ID, token_2022::ID as TOKEN_2022_ID, token_interface::Mint};

use glow_airspace::state::Airspace;

use crate::{
    events::TokenConfigured, seeds::TOKEN_CONFIG_SEED, ErrorCode, TokenAdmin, TokenConfig,
    TokenFeatures, TokenKind, MAX_CLAIM_VALUE_MODIFIER, MAX_COLLATERAL_VALUE_MODIFIER,
    MAX_TOKEN_STALENESS,
};

#[derive(AnchorDeserialize, AnchorSerialize, Debug, Eq, PartialEq, Clone)]
pub struct TokenConfigUpdate {
    /// The underlying token represented, if any
    pub underlying_mint: Pubkey,

    /// The underlying token's program
    pub underlying_mint_token_program: Pubkey,

    /// The administration authority for the token
    pub admin: TokenAdmin,

    /// Description of this token
    pub token_kind: TokenKind,

    /// A modifier to adjust the token value, based on the kind of token
    pub value_modifier: u16,

    /// The maximum staleness (seconds) that's acceptable for balances of this token.
    /// The staleness should be set depending on the type of adapter that this position will belong to.
    ///
    /// If the staleness is set to 0, it means that the token balance is always fresh.
    /// If the staleness is > 0, then the position's balance must be refreshed within the specified
    /// staleness period for it to be considered valid. This is important for positions of adapters
    /// that might act on behalf of the user without the margin program being aware.
    ///
    /// E.g. A claim on an external protocol, where that claim might be liquidated, leading to a change
    /// in position balance.
    ///
    /// The margin and margin pool programs correctly report their token balances, making the need to
    /// refresh balances periodically unnecessary. Thus for these programs, it is preferable to set
    /// the staleness to 0.
    pub max_staleness: u64,

    /// The token features.
    /// This featureset contains token restrictions, which might be applied to margin accounts
    /// whose featureset is incompatible with the token's featureset.
    pub token_features: TokenFeatures,
}

impl TokenConfigUpdate {
    pub fn check_modifier_limits(&self) -> Result<()> {
        // Ensure value modifier cannot exceed limit for all token kinds.
        // Use match in case we extend the token kinds which forces us to
        // update the match arms here.
        let val_modifier = self.value_modifier;
        match self.token_kind {
            TokenKind::Collateral | TokenKind::AdapterCollateral => {
                if val_modifier > MAX_COLLATERAL_VALUE_MODIFIER {
                    msg!(
                        "collateral value modifier cannot exceed limit, got: {}",
                        val_modifier
                    );
                    return err!(ErrorCode::InvalidConfigCollateralValueModifierLimit);
                }
            }
            TokenKind::Claim => {
                if val_modifier > MAX_CLAIM_VALUE_MODIFIER {
                    msg!(
                        "claim value modifier cannot exceed limit, got: {}",
                        val_modifier
                    );
                    return err!(ErrorCode::InvalidConfigClaimValueModifierLimit);
                }
            }
        }

        Ok(())
    }

    pub fn check_max_staleness(&self) -> Result<()> {
        // Ensure token balance staleness is below the maximum allowed.
        // As guidance, positions of adapters whose values can be changed externally (e.g. a perp on some other protocol)
        // should have a max_staleness > 0 to ensure that the token balance is not stale.
        // The margin and margin pool programs correctly report their token balances, making the need to
        // refresh balances periodically unnecessary.
        if self.max_staleness > MAX_TOKEN_STALENESS {
            msg!(
                "token balance staleness cannot exceed limit, got: {}",
                self.max_staleness
            );
            return err!(ErrorCode::InvalidConfigStaleness);
        }

        Ok(())
    }

    pub fn check_token_program(&self) -> Result<()> {
        let mint_program = self.underlying_mint_token_program;
        if mint_program == TOKEN_ID || mint_program == TOKEN_2022_ID {
            return Ok(());
        }

        msg!("unsupported token program: {:?}", mint_program);
        err!(ErrorCode::InvalidConfigTokenProgramUnsupported)
    }

    /// There are 3 token kinds and 2 admins:
    /// - Collateral, AdapterCollateral, Claim
    /// - Margin, Adapter
    ///
    /// Validations:
    /// - An Adapter can register any token
    ///     * Claim: registered through the adapter program via CPI (e.g. margin_pool::register_loan)
    ///     * Collateral: registered directly through the margin program (e.g. margin::register_position)
    ///     * AdapterCollateral: registered through the adapter program via CPI (e.g. test_service::register_adapter_position)
    /// - Margin can only register Collateral
    pub fn check_token_kind(&self) -> Result<()> {
        match self.admin {
            TokenAdmin::Margin { .. } => {
                msg!("Margin admin cannot own any token that is not Collateral");
                require!(
                    self.token_kind == TokenKind::Collateral,
                    ErrorCode::InvalidConfigTokenKind
                );
            }
            TokenAdmin::Adapter(_) => {
                // No-op, the adapter can own any position
            }
        }

        Ok(())
    }
}

#[derive(Accounts)]
pub struct ConfigureToken<'info> {
    /// The authority allowed to make changes to configuration
    pub authority: Signer<'info>,

    /// The airspace being modified
    #[account(has_one = authority)]
    pub airspace: Account<'info, Airspace>,

    /// The payer for any rent costs, if required
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The mint for the token being configured
    pub mint: InterfaceAccount<'info, Mint>,

    /// The config account to be modified
    #[account(
        init_if_needed,
        seeds = [
        TOKEN_CONFIG_SEED,
        airspace.key().as_ref(),
        mint.key().as_ref()
        ],
        bump,
        payer = payer,
        space = 8 + std::mem::size_of::<TokenConfig>(),
    )]
    pub token_config: Account<'info, TokenConfig>,

    pub system_program: Program<'info, System>,
}

pub fn configure_token_handler(
    ctx: Context<ConfigureToken>,
    updated_config: TokenConfigUpdate,
) -> Result<()> {
    let config = &mut ctx.accounts.token_config;

    emit!(TokenConfigured {
        airspace: ctx.accounts.airspace.key(),
        mint: ctx.accounts.mint.key(),
        update: Some(updated_config.clone()),
    });

    updated_config.check_max_staleness()?;
    updated_config.token_features.check_valid_configuration()?;
    updated_config.check_token_kind()?;
    updated_config.check_modifier_limits()?;
    updated_config.check_token_program()?;

    // If not the first time this is called
    if config.mint != Pubkey::default() {
        validate_immutability_constraints(&updated_config, config)?;
    }

    apply_config_update(
        config,
        &updated_config,
        ctx.accounts.mint.key(),
        *ctx.accounts.mint.to_account_info().owner,
        ctx.accounts.airspace.key(),
    )?;

    Ok(())
}

fn validate_immutability_constraints(
    updated_config: &TokenConfigUpdate,
    existing_config: &TokenConfig,
) -> Result<()> {
    existing_config.compare_token_kind_immutability(updated_config)?;
    existing_config.allow_admin_transition(updated_config)?;
    existing_config.compare_feature_immutability(updated_config)?;

    Ok(())
}

fn apply_config_update(
    config: &mut TokenConfig,
    updated_config: &TokenConfigUpdate,
    mint_key: Pubkey,
    mint_token_program: Pubkey,
    airspace_key: Pubkey,
) -> Result<()> {
    config.mint = mint_key;
    config.mint_token_program = mint_token_program;
    config.airspace = airspace_key;
    config.underlying_mint = updated_config.underlying_mint;
    config.underlying_mint_token_program = updated_config.underlying_mint_token_program;
    config.admin = updated_config.admin;
    config.token_kind = updated_config.token_kind;
    config.value_modifier = updated_config.value_modifier;
    config.max_staleness = updated_config.max_staleness;
    config.token_features = updated_config.token_features;

    Ok(())
}
