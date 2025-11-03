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
use glow_metadata::cpi::accounts::SetEntry;
use glow_metadata::program::Metadata;
use glow_metadata::{PositionTokenMetadata, TokenKind, TokenMetadata};
use glow_program_common::oracle::TokenPriceOracle;

use crate::{events, state::*};
use glow_margin::{ErrorCode, TokenFeatures, MAX_MANAGEMENT_FEE_RATE, MAX_TOKEN_STALENESS};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct TokenMetadataParams {
    /// Description of this token
    pub token_kind: TokenKind,

    /// The weight of the asset's value relative to other tokens when used as collateral.
    pub collateral_weight: u16,

    /// The maximum leverage allowed on loans for the token
    pub max_leverage: u16,

    /// The maximum staleness allowed for the position's oracle
    pub max_staleness: u64,

    /// The supported token features
    pub token_features: u16,
}

#[derive(Accounts)]
pub struct Configure<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The authority allowed to modify the pool, which must sign
    pub authority: Signer<'info>,

    /// The airspace that the pool is being modified in
    #[account(
        constraint = airspace.authority == authority.key(),
    )]
    pub airspace: Box<Account<'info, Airspace>>,

    /// The pool to be configured
    #[account(
        mut,
        has_one = token_mint,
        constraint = margin_pool.airspace == airspace.key(),
    )]
    pub margin_pool: Account<'info, MarginPool>,

    /// CHECK: The token mint is checked to be the underlying mint of the metadata accounts
    pub token_mint: UncheckedAccount<'info>,

    #[account(mut, has_one = token_mint, has_one = airspace)]
    pub token_metadata: Box<Account<'info, TokenMetadata>>,

    #[account(mut,
        has_one = airspace,
        constraint = deposit_metadata.underlying_token_mint == token_mint.key(),
        constraint = deposit_metadata.position_token_mint == margin_pool.deposit_note_mint,
        constraint = deposit_metadata.position_token_mint == deposit_note_mint.key(),
    )]
    pub deposit_metadata: Box<Account<'info, PositionTokenMetadata>>,

    #[account(mut,
        has_one = airspace,
        constraint = loan_metadata.underlying_token_mint == token_mint.key(),
        constraint = loan_metadata.position_token_mint == margin_pool.loan_note_mint,
        constraint = loan_metadata.position_token_mint == loan_note_mint.key(),
    )]
    pub loan_metadata: Box<Account<'info, PositionTokenMetadata>>,

    /// Used as the key account for updating the metadata entry
    /// CHECK: Checked against constraint to match the deposit metadata
    pub deposit_note_mint: AccountInfo<'info>,
    /// Used as the key account for updating the metadata entry
    /// CHECK: Checked against constraint to match the loan metadata
    pub loan_note_mint: AccountInfo<'info>,

    pub metadata_program: Program<'info, Metadata>,
    system_program: Program<'info, System>,
}

impl<'info> Configure<'info> {
    fn set_metadata_context(&self) -> CpiContext<'_, '_, '_, 'info, SetEntry<'info>> {
        CpiContext::new(
            self.metadata_program.to_account_info(),
            SetEntry {
                airspace: self.airspace.to_account_info(),
                metadata_account: self.token_metadata.to_account_info(),
                authority: self.authority.to_account_info(),
                payer: self.payer.to_account_info(),
                system_program: self.system_program.to_account_info(),
            },
        )
    }

    fn set_deposit_metadata_context(&self) -> CpiContext<'_, '_, '_, 'info, SetEntry<'info>> {
        CpiContext::new(
            self.metadata_program.to_account_info(),
            SetEntry {
                airspace: self.airspace.to_account_info(),
                metadata_account: self.deposit_metadata.to_account_info(),
                authority: self.authority.to_account_info(),
                payer: self.payer.to_account_info(),
                system_program: self.system_program.to_account_info(),
            },
        )
    }

    fn set_loan_metadata_context(&self) -> CpiContext<'_, '_, '_, 'info, SetEntry<'info>> {
        CpiContext::new(
            self.metadata_program.to_account_info(),
            SetEntry {
                airspace: self.airspace.to_account_info(),
                metadata_account: self.loan_metadata.to_account_info(),
                authority: self.authority.to_account_info(),
                payer: self.payer.to_account_info(),
                system_program: self.system_program.to_account_info(),
            },
        )
    }
}

pub fn configure_handler(
    ctx: Context<Configure>,
    metadata: Option<TokenMetadataParams>,
    config: Option<MarginPoolConfig>,
    oracle: Option<TokenPriceOracle>,
) -> Result<()> {
    let pool = &mut ctx.accounts.margin_pool;

    if let Some(new_config) = config {
        pool.config = new_config;

        // Verify that the utilization and borrow rates are sane in the sense that
        // they are representing values on a strictly increasing (monotonic) function
        let c = new_config;
        require!(
            c.utilization_rate_1 < c.utilization_rate_2,
            ErrorCode::InvalidConfigUtilRate,
        );
        require!(
            c.borrow_rate_0 < c.borrow_rate_1
                && c.borrow_rate_1 < c.borrow_rate_2
                && c.borrow_rate_2 < c.borrow_rate_3,
            ErrorCode::InvalidConfigBorrowRate,
        );

        // Check cap on management fee rate
        require!(
            c.management_fee_rate <= MAX_MANAGEMENT_FEE_RATE,
            ErrorCode::InvalidConfigManagementRate,
        );
    }

    if let Some(new_oracle) = &oracle {
        pool.token_price_oracle = *new_oracle;
        // SECURITY: The pool oracle should be updated at the same time as the
        // TokenMetadata so they remain in sync.
        // If we ever need to create and update TokenMetadata without having a
        // corresponding pool, then we should revisit this interface.

        let mut metadata = ctx.accounts.token_metadata.clone();
        let mut data = vec![];

        metadata.token_price_oracle = *new_oracle;

        metadata.try_serialize(&mut data)?;

        glow_metadata::cpi::set_entry(
            ctx.accounts.set_metadata_context(),
            ctx.accounts.token_mint.key(),
            0,
            data,
        )?;

        emit!(events::TokenMetadataConfigured {
            requester: ctx.accounts.payer.key(),
            authority: ctx.accounts.authority.key(),
            metadata_account: ctx.accounts.token_metadata.key(),
            metadata: metadata.into_inner(),
        })
    }

    emit!(events::PoolConfigured {
        margin_pool: ctx.accounts.margin_pool.key(),
        config: config.unwrap_or_default(),
        oracle: oracle.unwrap_or_default(),
    });

    if let Some(params) = metadata {
        let mut metadata = ctx.accounts.deposit_metadata.clone();
        let mut data = vec![];

        // TODO: not ideal that margin pool is returning margin errors!
        require!(
            params.max_staleness <= MAX_TOKEN_STALENESS,
            ErrorCode::InvalidConfigStaleness
        );

        // Check for token feature validity
        require!(
            TokenFeatures::from_bits(params.token_features)
                .unwrap()
                .is_valid(),
            ErrorCode::InvalidFeatureFlags
        );

        metadata.token_kind = params.token_kind;
        metadata.value_modifier = params.collateral_weight;
        metadata.max_staleness = params.max_staleness;
        metadata.token_features = params.token_features;

        metadata.try_serialize(&mut data)?;

        glow_metadata::cpi::set_entry(
            ctx.accounts.set_deposit_metadata_context(),
            ctx.accounts.deposit_note_mint.key(),
            0,
            data,
        )?;

        emit!(events::PositionTokenMetadataConfigured {
            requester: ctx.accounts.payer.key(),
            authority: ctx.accounts.authority.key(),
            metadata_account: ctx.accounts.deposit_metadata.key(),
            metadata: metadata.into_inner(),
        });

        let mut metadata = ctx.accounts.loan_metadata.clone();
        let mut data = vec![];

        metadata.token_kind = TokenKind::Claim;
        metadata.value_modifier = params.max_leverage;
        metadata.max_staleness = params.max_staleness;
        metadata.token_features = params.token_features;

        metadata.try_serialize(&mut data)?;

        glow_metadata::cpi::set_entry(
            ctx.accounts.set_loan_metadata_context(),
            ctx.accounts.loan_note_mint.key(),
            0,
            data,
        )?;

        emit!(events::PositionTokenMetadataConfigured {
            requester: ctx.accounts.payer.key(),
            authority: ctx.accounts.authority.key(),
            metadata_account: ctx.accounts.loan_metadata.key(),
            metadata: metadata.into_inner(),
        });
    }

    Ok(())
}
