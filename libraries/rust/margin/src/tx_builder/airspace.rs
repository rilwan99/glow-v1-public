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

use glow_instructions::{airspace::AirspaceDetails, margin_pool::MarginPoolIxBuilder, MintInfo};
use glow_program_common::oracle::TokenPriceOracle;
use solana_sdk::pubkey::Pubkey;

use crate::{
    ix_builder::{
        derive_governor_id, test_service::if_not_initialized, AirspaceIxBuilder,
        MarginConfigIxBuilder, MarginPoolConfiguration,
    },
    solana::transaction::TransactionBuilder,
};
use glow_margin::{TokenAdmin, TokenConfigUpdate, TokenFeatures, TokenKind};

/// Utility for constructing transactions for administrative functions on protocol
/// resources within an airspace.
pub struct AirspaceAdmin {
    /// The airspace this interacts with
    authority: Pubkey,
    payer: Pubkey,
    as_ix: AirspaceIxBuilder,
}

impl AirspaceAdmin {
    /// Create new builder with payer as authority, for a given airspace based on its seed
    pub fn new(airspace_seed: &str, payer: Pubkey, authority: Pubkey) -> Self {
        Self {
            payer,
            authority,
            as_ix: AirspaceIxBuilder::new(airspace_seed, payer, authority),
        }
    }

    /// Getter for the airspace address
    pub fn airspace(&self) -> Pubkey {
        self.as_ix.address()
    }

    /// Getter for the airspace details, with address and authority
    pub fn airspace_details(&self) -> &AirspaceDetails {
        (self).as_ix.airspace_manager()
    }

    /// Create this airspace
    pub fn create_airspace(&self, authority: Pubkey, is_restricted: bool) -> TransactionBuilder {
        vec![self.as_ix.create(authority, is_restricted)].into()
    }

    /// Create a permit for a user to be allowed to use this airspace
    pub fn issue_user_permit(&self, user: Pubkey) -> TransactionBuilder {
        vec![self.as_ix.permit_create(user)].into()
    }

    /// Revoke a previously issued permit for a user, preventing them from continuing to
    /// use airspace resources.
    pub fn revoke_user_permit(&self, user: Pubkey, issuer: Pubkey) -> TransactionBuilder {
        vec![self.as_ix.permit_revoke(user, issuer)].into()
    }

    /// Create a new margin pool for a given token
    pub fn create_margin_pool(&self, token_mint: MintInfo) -> TransactionBuilder {
        let margin_pool_ix_builder = MarginPoolIxBuilder::new(self.airspace(), token_mint);
        vec![margin_pool_ix_builder.create(self.authority, self.payer, None)].into()
    }

    /// Configure a margin pool for the given token.
    pub fn configure_margin_pool(
        &self,
        token_mint: MintInfo,
        config: &MarginPoolConfiguration,
    ) -> TransactionBuilder {
        let mut instructions = vec![];
        let margin_config_ix_builder =
            MarginConfigIxBuilder::new(self.airspace_details().clone(), self.payer);

        let pool = MarginPoolIxBuilder::new(self.airspace(), token_mint);

        instructions.push(pool.configure(self.authority, self.payer, config));

        if let Some(metadata) = &config.metadata {
            let mut deposit_note_config_update = TokenConfigUpdate {
                admin: TokenAdmin::Adapter(glow_margin_pool::ID),
                underlying_mint: token_mint.address,
                underlying_mint_token_program: token_mint.token_program(),
                token_kind: metadata.token_kind.into(),
                value_modifier: metadata.collateral_weight,
                max_staleness: metadata.max_staleness,
                token_features: TokenFeatures::from_bits(metadata.token_features).unwrap(),
            };

            let mut loan_note_config_update = TokenConfigUpdate {
                admin: TokenAdmin::Adapter(glow_margin_pool::ID),
                underlying_mint: token_mint.address,
                underlying_mint_token_program: token_mint.token_program(),
                token_kind: TokenKind::Claim,
                value_modifier: metadata.max_leverage,
                max_staleness: metadata.max_staleness,
                token_features: TokenFeatures::from_bits(metadata.token_features).unwrap(),
            };

            if let Some(metadata) = &config.metadata {
                deposit_note_config_update.token_kind = metadata.token_kind.into();
                deposit_note_config_update.value_modifier = metadata.collateral_weight;
                loan_note_config_update.value_modifier = metadata.max_leverage;
            }

            instructions.push(
                margin_config_ix_builder
                    .configure_token(pool.deposit_note_mint, deposit_note_config_update),
            );
            instructions.push(
                margin_config_ix_builder
                    .configure_token(pool.loan_note_mint, loan_note_config_update),
            );
        }

        instructions.into()
    }

    /// Configure deposits for a given token (when placed directly into a margin account)
    pub fn configure_margin_token_deposits(
        &self,
        underlying_mint: MintInfo,
        config: Option<TokenDepositsConfig>,
    ) -> TransactionBuilder {
        let margin_config_ix =
            MarginConfigIxBuilder::new(self.airspace_details().clone(), self.payer);
        let config_update = config.map(|config| TokenConfigUpdate {
            underlying_mint: underlying_mint.address,
            underlying_mint_token_program: underlying_mint.token_program(),
            token_kind: TokenKind::Collateral,
            value_modifier: config.collateral_weight,
            max_staleness: config.max_staleness,
            admin: TokenAdmin::Margin {
                oracle: config.oracle,
            },
            token_features: config.token_features,
        });

        vec![margin_config_ix.configure_token(underlying_mint.address, config_update.unwrap())]
            .into()
    }

    /// Configure an adapter that can be invoked through a margin account
    pub fn configure_margin_adapter(
        &self,
        adapter_program_id: Pubkey,
        is_adapter: bool,
    ) -> TransactionBuilder {
        let margin_config_ix =
            MarginConfigIxBuilder::new(self.airspace_details().clone(), self.payer);

        vec![margin_config_ix.configure_adapter(adapter_program_id, is_adapter)].into()
    }

    /// Configure an adapter that can be invoked through a margin account
    pub fn configure_margin_liquidator(
        &self,
        liquidator: Pubkey,
        is_liquidator: bool,
    ) -> TransactionBuilder {
        let margin_config_ix =
            MarginConfigIxBuilder::new(self.airspace_details().clone(), self.payer);

        vec![margin_config_ix.configure_liquidator(liquidator, is_liquidator)].into()
    }
}

/// Configuration for token deposits into margin accounts
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub struct TokenDepositsConfig {
    /// The oracle for the token
    pub oracle: TokenPriceOracle,

    /// Adjust the collateral value of deposits in the associated token
    pub collateral_weight: u16,

    /// Adjust the max staleness of the token oracle
    pub max_staleness: u64,

    /// Token features
    pub token_features: TokenFeatures,
}

/// Instructions required to initialize global state for the protocol. Sets up the minimum state
/// necessary to configure resources within the protocol.
///
/// This primarily sets up the root permissions for the protocol. Must be signed by the default
/// governing address for the protocol. When built with the `testing` feature, the first signer
/// to submit these instructions becomes set as the governor address.
pub fn global_initialize_instructions(payer: Pubkey) -> Vec<TransactionBuilder> {
    let as_ix = AirspaceIxBuilder::new("", payer, payer);

    vec![
        // if_not_initialized(get_control_authority_address(), ctrl_ix.create_authority()).into(),
        if_not_initialized(derive_governor_id(), as_ix.create_governor_id()).into(),
    ]
}
