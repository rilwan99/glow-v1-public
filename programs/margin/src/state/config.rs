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

use anchor_lang::{prelude::*, Owners};
use bitflags::bitflags;
use bytemuck::{Contiguous, Pod, Zeroable};
use glow_program_common::oracle::TokenPriceOracle;

use crate::{ErrorCode, TokenConfigUpdate};

/// The current [TokenConfig] version, created in June 2025.
///
/// Version 1 was the original untagged version.
pub const TOKEN_CONFIG_VERSION: u8 = 2;

/// Description of the token's usage
#[derive(AnchorSerialize, AnchorDeserialize, Contiguous, Eq, PartialEq, Clone, Copy, Debug)]
#[repr(u32)]
pub enum TokenKind {
    /// The token can be used as collateral
    Collateral = 1,

    /// The token represents a debt that needs to be repaid
    Claim,

    /// The token balance is managed by a trusted adapter to represent the amount of collateral
    /// custodied by that adapter. The token account is owned by the adapter. Collateral
    /// is accessed through instructions to the adapter.
    AdapterCollateral,
}

impl Default for TokenKind {
    fn default() -> TokenKind {
        Self::Collateral
    }
}

impl From<glow_metadata::TokenKind> for TokenKind {
    fn from(kind: glow_metadata::TokenKind) -> Self {
        match kind {
            glow_metadata::TokenKind::NonCollateral => Self::Collateral,
            glow_metadata::TokenKind::Collateral => Self::Collateral,
            glow_metadata::TokenKind::Claim => Self::Claim,
            glow_metadata::TokenKind::AdapterCollateral => Self::AdapterCollateral,
        }
    }
}

/// Token features to allow the program to enforce the kinds of positions that can
/// be registered by a margin account.
///
/// For example, a margin account might only be restricted to tokens that are USD
/// derivatives, and preventing any other token from being registered.
#[derive(
    Zeroable, Pod, Debug, Eq, PartialEq, Default, AnchorSerialize, AnchorDeserialize, Clone, Copy,
)]
#[repr(transparent)]
pub struct TokenFeatures(u16);

bitflags! {
    impl TokenFeatures: u16 {
        /// Token restrictions should be enforced.
        const RESTRICTED                = 1 << 0;

        /// The token is a USD denominated stablecoin.
        const USD_STABLECOIN            = 1 << 1;

        /// The token is SOL or a SOL based LST.
        const SOL_BASED                 = 1 << 2;

        /// The token is WBTC or a WBTC derivative.
        const WBTC_BASED                = 1 << 3;
    }
}

impl TokenFeatures {
    /// Check if the token features are valid.
    /// Features are valid if only one flag at most (excluding `RESTRICTED`) is set.
    pub fn is_valid(&self) -> bool {
        let mut features = *self;
        features.remove(Self::RESTRICTED);
        features.bits().count_ones() <= 1
    }

    pub fn check_valid_configuration(&self) -> Result<()> {
        // Check that token features are known
        require!(
            TokenFeatures::from_bits(self.bits()).is_some(),
            ErrorCode::InvalidFeatureFlags,
        );

        // Validation: A token should have only 1 feature at a time, and can't be restricted with no other features
        require!(
            *self != TokenFeatures::RESTRICTED,
            ErrorCode::InvalidFeatureFlags,
        );

        // For token features, the RESTRICTED flag can be toggled as needed.
        // For subsequent validation, we get a copy of the flags and reset the restriction.
        let mut new_features = *self;
        new_features.set(TokenFeatures::RESTRICTED, false); // Clear the first bit

        // Validation: At most only one feature flag can be set
        require!(
            new_features.bits().count_ones() <= 1,
            ErrorCode::InvalidFeatureFlags
        );

        Ok(())
    }
}

mod _idl {
    use super::*;

    #[derive(Zeroable, AnchorSerialize, AnchorDeserialize, Default)]
    pub struct TokenFeatures {
        pub flags: u16,
    }
}

/// The configuration account specifying parameters for a token when used
/// in a position within a margin account.
#[account]
#[derive(Debug, Eq, PartialEq)]
pub struct TokenConfig {
    /// The mint for the token
    pub mint: Pubkey,

    /// The token program of the mint
    pub mint_token_program: Pubkey,

    /// The mint for the underlying token represented, if any
    pub underlying_mint: Pubkey,

    /// The program of the underlying token represented, if any
    pub underlying_mint_token_program: Pubkey,

    /// The space this config is valid within
    pub airspace: Pubkey,

    /// Description of this token
    ///
    /// This determines the way the margin program values a token as a position in a
    /// margin account.
    pub token_kind: TokenKind,

    /// A modifier to adjust the token value, based on the kind of token
    pub value_modifier: u16,

    /// The maximum staleness (seconds) that's acceptable for balances of this token
    pub max_staleness: u64,

    /// The administrator of this token, which has the authority to provide information
    /// about (e.g. prices) and otherwise modify position states for these tokens.
    pub admin: TokenAdmin,

    /// Features and restrictions about the token (in the airspace).
    pub token_features: TokenFeatures,

    /// The version of the token config. Introduced in June 2025.
    pub version: u8,

    // /// Bytes that are reserved for future versions
    pub reserved: [u8; 64],
}

impl Owners for TokenConfig {
    fn owners() -> &'static [Pubkey] {
        static OWNERS: [Pubkey; 1] = [crate::ID];
        &OWNERS
    }
}

impl PartialEq<TokenConfigUpdate> for TokenConfig {
    fn eq(&self, other: &TokenConfigUpdate) -> bool {
        self.underlying_mint == other.underlying_mint
            && self.admin == other.admin
            && self.token_kind == other.token_kind
            && self.value_modifier == other.value_modifier
            && self.max_staleness == other.max_staleness
            && self.token_features == other.token_features
    }
}

impl TokenConfig {
    // pub const SPACE: usize = 8 + 2 + std::mem::size_of::<Self>();

    pub fn compare_token_kind_immutability(&self, other: &TokenConfigUpdate) -> Result<()> {
        if self.token_kind != other.token_kind {
            msg!("token kind cannot be changed");
            return err!(ErrorCode::InvalidConfigTokenKind);
        }

        if self.underlying_mint == Pubkey::default() {
            msg!("the underlying mint must be set");
            return err!(ErrorCode::InvalidConfigUnderlyingMintEmpty);
        }

        // Ensure underlying mint cannot be changed if already set
        if other.underlying_mint != self.underlying_mint {
            msg!("underlying mint cannot be changed");
            return err!(ErrorCode::InvalidConfigUnderlyingMintChange);
        }

        Ok(())
    }

    pub fn adapter_program(&self) -> Option<Pubkey> {
        match self.admin {
            TokenAdmin::Adapter(address) => Some(address),
            _ => None,
        }
    }

    pub fn oracle(&self) -> Option<TokenPriceOracle> {
        match self.admin {
            TokenAdmin::Margin { oracle } => Some(oracle),
            _ => None,
        }
    }

    pub fn compare_feature_immutability(&self, other: &TokenConfigUpdate) -> Result<()> {
        let mut existing_stripped = self.token_features;
        existing_stripped.set(TokenFeatures::RESTRICTED, false);

        let mut new_stripped = other.token_features;
        new_stripped.set(TokenFeatures::RESTRICTED, false);

        // Validation: if the token features (except restriction) were set, prevent updating them.
        // [TokenFeatures::RESTRICTED] can however be toggled, as there might be a need to mark a
        // token as restricted after it has been created, or to lift that restriction.
        //
        // NOTE: We have to be very careful not to get it wrong when setting them up, as this will
        // prevent us from changing the features arbitrarily if we get them wrong.
        if !existing_stripped.is_empty() {
            // We should not be able to clear feature flags because then we could clear them,
            // and set them to a different value in a subsequent invocation of this instruction.
            require!(
                new_stripped == existing_stripped,
                ErrorCode::InvalidFeatureFlags
            );
        }

        Ok(())
    }

    pub fn allow_admin_transition(&self, update: &TokenConfigUpdate) -> Result<()> {
        match self.admin {
            // Allow token admin changes if it replaces an existing oracle.
            // Disallow changing the admin (enum) type.
            //
            // If the current admin is of type:
            //   - Adapter: disallow changing its address
            //   - Margin: allow changing the (oracle source) address
            TokenAdmin::Adapter(current) => match update.admin {
                TokenAdmin::Adapter(new) => {
                    // The adapter address is immutable
                    if current != new {
                        return err!(ErrorCode::InvalidConfigAdapterAddressChange);
                    }
                }
                TokenAdmin::Margin { .. } => {
                    // We do not allow the change from Adapter to Margin
                    return err!(ErrorCode::InvalidConfigAdapterToMargin);
                }
            },
            TokenAdmin::Margin { .. } => match update.admin {
                TokenAdmin::Adapter { .. } => {
                    // We do not allow the change from Margin to Adapter
                    return err!(ErrorCode::InvalidConfigMarginToAdapter);
                }
                TokenAdmin::Margin { .. } => {
                    // This is fine since the oracle source address can be modified
                }
            },
        }

        Ok(())
    }
}

/// Description of which program administers a token
#[derive(AnchorSerialize, AnchorDeserialize, Debug, Eq, PartialEq, Clone, Copy)]
pub enum TokenAdmin {
    /// This margin program administers the token directly
    Margin {
        /// An oracle that can be used to collect price information for a token
        oracle: TokenPriceOracle,
    },

    /// The token is administered by the given adapter program
    ///
    /// The adapter is responsible for providing price information for the token.
    Adapter(Pubkey),
}

/// Configuration enabling a signer to execute permissioned actions
#[account]
#[derive(Default, Debug, Eq, PartialEq)]
pub struct Permit {
    /// Airspace where the permit is valid.
    pub airspace: Pubkey,

    /// Address which may sign to perform the permitted actions.
    pub owner: Pubkey,

    /// Actions which may be performed with the signature of the owner.
    pub permissions: Permissions,
}

impl Permit {
    pub fn validate(
        &self,
        airspace: Pubkey,
        owner: Pubkey,
        permissions: Permissions,
    ) -> Result<()> {
        if airspace != self.airspace {
            msg!(
                "provided airspace: {airspace} - permit's airspace: {}",
                self.airspace
            );
            return err!(ErrorCode::WrongAirspace);
        }
        if owner != self.owner {
            msg!("provided owner: {owner} - permit's owner: {}", self.owner);
            return err!(ErrorCode::PermitNotOwned);
        }
        if !self.permissions.contains(permissions) {
            msg!("permissions: {:?}", self.permissions);
            return err!(ErrorCode::InsufficientPermissions);
        }

        Ok(())
    }
}

/// Actions in the margin program that require special approval from an
/// airspace authority before an address is authorized to sign for the
/// instruction performing this action.
#[derive(Debug, Eq, PartialEq, Default, AnchorSerialize, AnchorDeserialize, Clone, Copy)]
#[repr(transparent)]
pub struct Permissions(u32);

bitflags! {
    impl Permissions: u32 {
        /// Liquidate margin accounts in this airspace.
        const LIQUIDATE                 = 1 << 0;

        /// Execute update_position_metadata for margin accounts in this airspace.
        const REFRESH_POSITION_CONFIG   = 1 << 1;

        /// Can operate margin vaults
        const OPERATE_VAULTS            = 1 << 2;
    }
}

/// Configuration for allowed adapters
#[account]
#[derive(Default, Debug, Eq, PartialEq)]
pub struct AdapterConfig {
    /// The airspace this adapter can be used in
    pub airspace: Pubkey,

    /// The program address allowed to be called as an adapter
    pub adapter_program: Pubkey,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_spl::token_2022::ID as TOKEN_2022_ID;

    #[test]
    fn test_token_features_is_valid_ok() {
        let valid_features = vec![
            // Single feature flag
            TokenFeatures::empty(),
            TokenFeatures::RESTRICTED,
            TokenFeatures::USD_STABLECOIN,
            TokenFeatures::SOL_BASED,
            TokenFeatures::WBTC_BASED,
            // Single feature with RESTRICTED
            TokenFeatures::USD_STABLECOIN | TokenFeatures::RESTRICTED,
            TokenFeatures::SOL_BASED | TokenFeatures::RESTRICTED,
            TokenFeatures::WBTC_BASED | TokenFeatures::RESTRICTED,
        ];
        for feature in valid_features {
            assert!(feature.is_valid());
        }
    }

    #[test]
    fn test_token_features_is_valid_error() {
        let invalid_features = vec![
            // Multiple feature flags without RESTRICTED
            TokenFeatures::USD_STABLECOIN | TokenFeatures::SOL_BASED,
            TokenFeatures::USD_STABLECOIN | TokenFeatures::WBTC_BASED,
            TokenFeatures::SOL_BASED | TokenFeatures::WBTC_BASED,
            // Multiple feature flags with RESTRICTED
            TokenFeatures::USD_STABLECOIN | TokenFeatures::SOL_BASED | TokenFeatures::RESTRICTED,
            // All flags without RESTRICTED
            TokenFeatures::USD_STABLECOIN | TokenFeatures::SOL_BASED | TokenFeatures::WBTC_BASED,
            // All flags
            TokenFeatures::USD_STABLECOIN
                | TokenFeatures::SOL_BASED
                | TokenFeatures::WBTC_BASED
                | TokenFeatures::RESTRICTED,
        ];
        for feature in invalid_features {
            assert!(!feature.is_valid());
        }
    }

    #[test]
    fn test_token_features_check_valid_configuration_ok() {
        let valid_configurations = vec![
            TokenFeatures::empty(),
            TokenFeatures::USD_STABLECOIN,
            TokenFeatures::SOL_BASED,
            TokenFeatures::USD_STABLECOIN | TokenFeatures::RESTRICTED,
            TokenFeatures::WBTC_BASED | TokenFeatures::RESTRICTED,
        ];

        for config in valid_configurations {
            assert!(config.check_valid_configuration().is_ok());
        }
    }

    #[test]
    fn test_token_features_check_valid_configuration_error() {
        let invalid_features = vec![
            // Only RESTRICTED flag (not allowed)
            TokenFeatures::RESTRICTED,
            // Multiple feature flags without RESTRICTED
            TokenFeatures::USD_STABLECOIN | TokenFeatures::SOL_BASED,
            TokenFeatures::USD_STABLECOIN | TokenFeatures::WBTC_BASED,
            TokenFeatures::SOL_BASED | TokenFeatures::WBTC_BASED,
            // Multiple feature flags with RESTRICTED
            TokenFeatures::USD_STABLECOIN | TokenFeatures::SOL_BASED | TokenFeatures::RESTRICTED,
            // All flags without RESTRICTED
            TokenFeatures::USD_STABLECOIN | TokenFeatures::SOL_BASED | TokenFeatures::WBTC_BASED,
            // All flags
            TokenFeatures::USD_STABLECOIN
                | TokenFeatures::SOL_BASED
                | TokenFeatures::WBTC_BASED
                | TokenFeatures::RESTRICTED,
            // Unknown flags
            TokenFeatures(0b10000),
            TokenFeatures(0b100000),
            TokenFeatures(0b1000000),
        ];

        for feature in invalid_features {
            assert!(feature.check_valid_configuration().is_err());
        }
    }

    fn create_test_token_config_update(
        token_kind: TokenKind,
        underlying_mint: Pubkey,
    ) -> TokenConfigUpdate {
        TokenConfigUpdate {
            underlying_mint,
            underlying_mint_token_program: TOKEN_2022_ID,
            admin: TokenAdmin::Margin {
                oracle: TokenPriceOracle::NoOracle,
            },
            token_kind,
            value_modifier: 8000,
            max_staleness: 300,
            token_features: TokenFeatures::empty(),
        }
    }

    #[test]
    fn test_compare_token_kind_immutability_ok() {
        let underlying_mint = Pubkey::new_unique();

        let success_cases = vec![
            // Same token kind and mint
            (
                create_test_token_config(TokenKind::Collateral, underlying_mint),
                create_test_token_config_update(TokenKind::Collateral, underlying_mint),
            ),
            (
                create_test_token_config(TokenKind::Claim, underlying_mint),
                create_test_token_config_update(TokenKind::Claim, underlying_mint),
            ),
            (
                create_test_token_config(TokenKind::AdapterCollateral, underlying_mint),
                create_test_token_config_update(TokenKind::AdapterCollateral, underlying_mint),
            ),
        ];

        for (config, update) in success_cases {
            assert!(config.compare_token_kind_immutability(&update).is_ok());
        }
    }

    #[test]
    fn test_compare_token_kind_immutability_error() {
        let underlying_mint1 = Pubkey::new_unique();
        let underlying_mint2 = Pubkey::new_unique();

        let error_cases = vec![
            // Different token kinds
            (
                create_test_token_config(TokenKind::Collateral, underlying_mint1),
                create_test_token_config_update(TokenKind::Claim, underlying_mint1),
            ),
            (
                create_test_token_config(TokenKind::Claim, underlying_mint1),
                create_test_token_config_update(TokenKind::AdapterCollateral, underlying_mint1),
            ),
            // Underlying mint is default
            (
                create_test_token_config(TokenKind::Collateral, Pubkey::default()),
                create_test_token_config_update(TokenKind::Collateral, Pubkey::default()),
            ),
            // Different underlying mints
            (
                create_test_token_config(TokenKind::Collateral, underlying_mint1),
                create_test_token_config_update(TokenKind::Collateral, underlying_mint2),
            ),
        ];

        for (config, update) in error_cases {
            assert!(config.compare_token_kind_immutability(&update).is_err());
        }
    }

    #[test]
    fn test_compare_feature_immutability_ok() {
        let underlying_mint = Pubkey::new_unique();

        let success_cases = vec![
            // Empty features can be updated to any single feature
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::empty(),
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::USD_STABLECOIN,
                ),
            ),
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::empty(),
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::SOL_BASED,
                ),
            ),
            // Same feature flags (non-RESTRICTED) should be allowed
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::USD_STABLECOIN,
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::USD_STABLECOIN,
                ),
            ),
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::SOL_BASED,
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::SOL_BASED,
                ),
            ),
            // RESTRICTED flag can be toggled on existing features
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::USD_STABLECOIN,
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::USD_STABLECOIN | TokenFeatures::RESTRICTED,
                ),
            ),
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::SOL_BASED | TokenFeatures::RESTRICTED,
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::SOL_BASED,
                ),
            ),
        ];

        for (config, update) in success_cases {
            assert!(config.compare_feature_immutability(&update).is_ok());
        }
    }

    #[test]
    fn test_compare_feature_immutability_error() {
        let underlying_mint = Pubkey::new_unique();

        let error_cases = vec![
            // Cannot change from one feature to another
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::USD_STABLECOIN,
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::SOL_BASED,
                ),
            ),
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::SOL_BASED,
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::WBTC_BASED,
                ),
            ),
            // Cannot clear existing feature flags
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::USD_STABLECOIN,
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::empty(),
                ),
            ),
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::SOL_BASED,
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::empty(),
                ),
            ),
            // Cannot add additional features
            (
                create_test_token_config_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::USD_STABLECOIN,
                ),
                create_test_token_config_update_with_features(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenFeatures::USD_STABLECOIN | TokenFeatures::SOL_BASED,
                ),
            ),
        ];

        for (config, update) in error_cases {
            assert!(config.compare_feature_immutability(&update).is_err());
        }
    }

    #[test]
    fn test_allow_admin_transition_ok() {
        let underlying_mint = Pubkey::new_unique();
        let adapter_address = Pubkey::new_unique();

        let success_cases = vec![
            // Adapter to Adapter with same address
            (
                create_test_token_config_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Adapter(adapter_address),
                ),
                create_test_token_config_update_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Adapter(adapter_address),
                ),
            ),
            // Margin to Margin with same oracle
            (
                create_test_token_config_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Margin {
                        oracle: TokenPriceOracle::PythPull { feed_id: [2u8; 32] },
                    },
                ),
                create_test_token_config_update_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Margin {
                        oracle: TokenPriceOracle::PythPull { feed_id: [2u8; 32] },
                    },
                ),
            ),
            // Margin to Margin with different oracle
            (
                create_test_token_config_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Margin {
                        oracle: TokenPriceOracle::NoOracle,
                    },
                ),
                create_test_token_config_update_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Margin {
                        oracle: TokenPriceOracle::PythPull { feed_id: [3u8; 32] },
                    },
                ),
            ),
        ];

        for (config, update) in success_cases {
            assert!(config.allow_admin_transition(&update).is_ok());
        }
    }

    #[test]
    fn test_allow_admin_transition_error() {
        let underlying_mint = Pubkey::new_unique();
        let adapter_address1 = Pubkey::new_unique();
        let adapter_address2 = Pubkey::new_unique();

        let error_cases = vec![
            // Adapter to Adapter with different address
            (
                create_test_token_config_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Adapter(adapter_address1),
                ),
                create_test_token_config_update_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Adapter(adapter_address2),
                ),
            ),
            // Adapter to Margin
            (
                create_test_token_config_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Adapter(adapter_address1),
                ),
                create_test_token_config_update_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Margin {
                        oracle: TokenPriceOracle::NoOracle,
                    },
                ),
            ),
            // Collateral to Adapter
            (
                create_test_token_config_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Margin {
                        oracle: TokenPriceOracle::NoOracle,
                    },
                ),
                create_test_token_config_update_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Adapter(adapter_address1),
                ),
            ),
            // Margin to Adapter
            (
                create_test_token_config_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Margin {
                        oracle: TokenPriceOracle::PythPull { feed_id: [4u8; 32] },
                    },
                ),
                create_test_token_config_update_with_admin(
                    TokenKind::Collateral,
                    underlying_mint,
                    TokenAdmin::Adapter(adapter_address2),
                ),
            ),
        ];

        for (config, update) in error_cases {
            assert!(config.allow_admin_transition(&update).is_err());
        }
    }

    fn create_test_token_config(token_kind: TokenKind, underlying_mint: Pubkey) -> TokenConfig {
        TokenConfig {
            mint: Pubkey::new_unique(),
            mint_token_program: TOKEN_2022_ID,
            underlying_mint,
            underlying_mint_token_program: TOKEN_2022_ID,
            airspace: Pubkey::new_unique(),
            token_kind,
            value_modifier: 8000,
            max_staleness: 300,
            admin: TokenAdmin::Margin {
                oracle: TokenPriceOracle::NoOracle,
            },
            token_features: TokenFeatures::empty(),
            version: TOKEN_CONFIG_VERSION,
            reserved: [0; 64],
        }
    }

    fn create_test_token_config_with_admin(
        token_kind: TokenKind,
        underlying_mint: Pubkey,
        admin: TokenAdmin,
    ) -> TokenConfig {
        TokenConfig {
            mint: Pubkey::new_unique(),
            mint_token_program: TOKEN_2022_ID,
            underlying_mint,
            underlying_mint_token_program: TOKEN_2022_ID,
            airspace: Pubkey::new_unique(),
            token_kind,
            value_modifier: 8000,
            max_staleness: 300,
            admin,
            token_features: TokenFeatures::empty(),
            version: TOKEN_CONFIG_VERSION,
            reserved: [0; 64],
        }
    }

    fn create_test_token_config_update_with_admin(
        token_kind: TokenKind,
        underlying_mint: Pubkey,
        admin: TokenAdmin,
    ) -> TokenConfigUpdate {
        TokenConfigUpdate {
            underlying_mint,
            underlying_mint_token_program: TOKEN_2022_ID,
            admin,
            token_kind,
            value_modifier: 8000,
            max_staleness: 300,
            token_features: TokenFeatures::empty(),
        }
    }

    fn create_test_token_config_with_features(
        token_kind: TokenKind,
        underlying_mint: Pubkey,
        features: TokenFeatures,
    ) -> TokenConfig {
        TokenConfig {
            mint: Pubkey::new_unique(),
            mint_token_program: TOKEN_2022_ID,
            underlying_mint,
            underlying_mint_token_program: TOKEN_2022_ID,
            airspace: Pubkey::new_unique(),
            token_kind,
            value_modifier: 8000,
            max_staleness: 300,
            admin: TokenAdmin::Margin {
                oracle: TokenPriceOracle::NoOracle,
            },
            token_features: features,
            version: TOKEN_CONFIG_VERSION,
            reserved: [0; 64],
        }
    }

    fn create_test_token_config_update_with_features(
        token_kind: TokenKind,
        underlying_mint: Pubkey,
        features: TokenFeatures,
    ) -> TokenConfigUpdate {
        TokenConfigUpdate {
            underlying_mint,
            underlying_mint_token_program: TOKEN_2022_ID,
            admin: TokenAdmin::Margin {
                oracle: TokenPriceOracle::NoOracle,
            },
            token_kind,
            value_modifier: 8000,
            max_staleness: 300,
            token_features: features,
        }
    }
}
