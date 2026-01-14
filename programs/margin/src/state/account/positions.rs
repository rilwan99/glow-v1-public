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

use std::{convert::TryFrom, result::Result};

use anchor_lang::Result as AnchorResult;
use anchor_lang::{prelude::*, Owners};
use bytemuck::{Contiguous, Pod, Zeroable};
use glow_program_common::Number128;
use glow_program_proc_macros::assert_size;
#[cfg(any(test, feature = "cli"))]
use serde::ser::{Serialize, SerializeStruct, Serializer};

use super::Approver;

use crate::{ErrorCode, TokenConfig, TokenFeatures, TokenKind};

const POS_PRICE_VALID: u8 = 1;

#[assert_size(24)]
#[derive(
    Pod, Zeroable, AnchorSerialize, AnchorDeserialize, Debug, Default, Clone, Copy, Eq, PartialEq,
)]
#[cfg_attr(
    any(test, feature = "cli"),
    derive(serde::Serialize),
    serde(rename_all = "camelCase")
)]
#[repr(C)]
pub struct PriceInfo {
    /// The current price
    pub value: i64,

    /// The timestamp the price was valid at
    pub timestamp: u64,

    /// The exponent for the price value
    pub exponent: i32,

    /// Flag indicating if the price is valid for the position
    pub is_valid: u8,

    #[cfg_attr(any(test, feature = "cli"), serde(skip_serializing))]
    pub _reserved: [u8; 3],
}

impl PriceInfo {
    pub fn new_valid(exponent: i32, value: i64, timestamp: u64) -> Self {
        Self {
            value,
            exponent,
            timestamp,
            is_valid: POS_PRICE_VALID,
            _reserved: [0u8; 3],
        }
    }

    pub fn new_invalid() -> Self {
        Self {
            value: 0,
            exponent: 0,
            timestamp: 0,
            is_valid: 0,
            _reserved: [0u8; 3],
        }
    }

    pub fn is_valid(&self) -> bool {
        self.is_valid == POS_PRICE_VALID
    }

    /// Convert to [Number128] if the price is valid
    pub fn to_number128(&self) -> AnchorResult<Number128> {
        if self.is_valid() {
            Ok(Number128::from_decimal(self.value, self.exponent))
        } else {
            err!(crate::ErrorCode::InvalidPrice)
        }
    }
}

#[assert_size(192)]
#[derive(Pod, Zeroable, AnchorSerialize, AnchorDeserialize, Default, Clone, Copy)]
#[repr(C)]
pub struct AccountPosition {
    /// The address of the token/mint of the asset
    pub token: Pubkey,

    /// The address of the account holding the tokens.
    pub address: Pubkey,

    /// The address of the adapter managing the asset
    pub adapter: Pubkey,

    /// The current value of this position, stored as a `Number128` with fixed precision.
    pub value: [u8; 16],

    /// The amount of tokens in the account
    pub balance: u64,

    /// The timestamp of the last balance update
    pub balance_timestamp: u64,

    /// The current price/value of each token
    pub price: PriceInfo,

    /// The kind of balance this position contains
    pub kind: u32,

    /// The exponent for the token value
    pub exponent: i16,

    /// A weight on the value of this asset when counting collateral
    pub value_modifier: u16,

    /// The max staleness for the account balance (seconds)
    pub max_staleness: u64,

    /// Flags that are set by the adapter
    pub flags: AdapterPositionFlags,

    /// Token program of position
    /// 0 = legacy SPL format
    /// 1 = token-2022 format
    pub is_token_2022: u8,

    /// Token feature flags inherited by the position when it was created
    pub token_features: TokenFeatures,

    /// Unused
    pub _reserved: [u8; 20],
}

#[repr(transparent)]
#[derive(Zeroable, Pod, AnchorSerialize, AnchorDeserialize, Default, Debug, Clone, Copy)]
pub struct AdapterPositionFlags(u8);

bitflags::bitflags! {
    impl AdapterPositionFlags: u8 {
        /// The position may never be removed by the user, even if the balance remains at zero,
        /// until the adapter explicitly unsets this flag.
        const REQUIRED = 1 << 0;

        /// Only applies to claims.
        /// For any other position, this can be set, but it will be ignored.
        /// The claim must be repaid immediately.
        /// The account will be considered unhealty if there is any balance on this position.
        const PAST_DUE = 1 << 1;
    }
}

mod _idl {
    use super::*;

    #[derive(Zeroable, AnchorSerialize, AnchorDeserialize, Default)]
    pub struct AdapterPositionFlags {
        pub flags: u8,
    }
}

impl AccountPosition {
    pub fn kind(&self) -> TokenKind {
        TokenKind::from_integer(self.kind).unwrap_or_default()
    }

    // Raw USD value of a position (balance * price)
    pub fn calculate_value(&mut self) {
        self.value = (Number128::from_decimal(self.balance, self.exponent)
            * Number128::from_decimal(self.price.value, self.price.exponent))
        .into_bits();
    }

    pub fn value(&self) -> Number128 {
        Number128::from_bits(self.value)
    }

    pub fn collateral_value(&self) -> Number128 {
        assert!(
            self.kind() == TokenKind::Collateral || self.kind() == TokenKind::AdapterCollateral
        );

        Number128::from_decimal(self.value_modifier, -2) * self.value()
    }

    pub fn required_collateral_value(&self) -> Number128 {
        assert_eq!(self.kind(), TokenKind::Claim);

        let modifier = Number128::from_decimal(self.value_modifier, -2);

        if modifier == Number128::ZERO {
            msg!("no leverage configured for claim {}", &self.token);
            Number128::MAX
        } else {
            //  debt / leverage = required collateral
            self.value() / modifier
        }
    }

    /// Update the balance for this position
    pub fn set_balance(&mut self, balance: u64, timestamp: u64) {
        self.balance = balance;
        self.balance_timestamp = timestamp;
        self.calculate_value();
    }

    /// Update the price for this position
    pub fn set_price(&mut self, price: &PriceInfo) -> Result<(), ErrorCode> {
        self.price = *price;
        self.calculate_value();

        Ok(())
    }

    // Auth check that determines whether a position can be registered/closed
    // based on (1) caller and (2) type of position
    pub fn may_be_registered_or_closed(&self, approvals: &[Approver]) -> bool {
        let mut authority_approved = false;
        let mut adapter_approved = false;

        for approval in approvals {
            match approval {
                Approver::MarginAccountAuthority => authority_approved = true,
                Approver::Adapter(approving_adapter) => {
                    adapter_approved = *approving_adapter == self.adapter
                }
            }
        }

        match self.kind() {
            // Only Authority can close it
            TokenKind::Collateral => authority_approved && !adapter_approved,
            // Requires Authority AND matching Adapter
            TokenKind::Claim | TokenKind::AdapterCollateral => {
                authority_approved && adapter_approved
            }
        }
    }
}

impl std::fmt::Debug for AccountPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        let mut acc = f.debug_struct("AccountPosition");
        acc.field("token", &self.token)
            .field("address", &self.address)
            .field("adapter", &self.adapter)
            .field("value", &self.value().to_string())
            .field("balance", &self.balance)
            .field("balance_timestamp", &self.balance_timestamp)
            .field("price", &self.price)
            .field("kind", &self.kind())
            .field("exponent", &self.exponent)
            .field("value_modifier", &self.value_modifier)
            .field("flags", &self.flags)
            .field("max_staleness", &self.max_staleness)
            .field("is_token_2022", &self.is_token_2022)
            .field("token_features", &self.token_features);

        acc.finish()
    }
}

#[cfg(any(test, feature = "cli"))]
impl Serialize for TokenKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match *self {
            TokenKind::Claim => "Claim",
            TokenKind::Collateral => "Collateral",
            TokenKind::AdapterCollateral => "AdapterCollateral",
        })
    }
}

#[cfg(any(test, feature = "cli"))]
impl Serialize for AccountPosition {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("AccountPosition", 11)?;
        s.serialize_field("address", &self.address.to_string())?;
        s.serialize_field("token", &self.token.to_string())?;
        s.serialize_field("adapter", &self.adapter.to_string())?;
        s.serialize_field("value", &self.value().to_string())?;
        s.serialize_field("balance", &self.balance)?;
        s.serialize_field("balanceTimestamp", &self.balance_timestamp)?;
        s.serialize_field("price", &self.price)?;
        s.serialize_field("kind", &self.kind())?;
        s.serialize_field("exponent", &self.exponent)?;
        s.serialize_field("valueModifier", &self.value_modifier)?;
        s.serialize_field("maxStaleness", &self.max_staleness)?;
        s.serialize_field("isToken2022", &self.is_token_2022)?;
        s.serialize_field("tokenFeatures", &self.token_features.bits())?;
        s.serialize_field("flags", &self.flags.bits())?;
        s.end()
    }
}

#[assert_size(40)]
#[derive(AnchorSerialize, AnchorDeserialize, Default, Pod, Zeroable, Debug, Clone, Copy)]
#[repr(C)]
pub struct AccountPositionKey {
    /// The address of the mint for the position token
    pub mint: Pubkey,

    /// The array index where the data for this position is located
    pub index: u64,
}

#[assert_size(7432)]
#[derive(AnchorSerialize, AnchorDeserialize, Default, Pod, Zeroable, Debug, Clone, Copy)]
#[repr(C)]
pub struct AccountPositionList {
    pub length: u64,
    pub map: [AccountPositionKey; 32],
    pub positions: [AccountPosition; 32],
}

impl AccountPositionList {
    /// Add a position to the position list.
    ///
    /// If the position does not exist, Finds an empty slot in `map` and
    /// `positions`, adds an empty position to the slot, and returns a mutable
    /// reference to the position which must be initialized with the correct
    /// data.
    ///
    /// If the position already exists, returns the key only, and no mutable
    /// position.
    pub fn add(
        &mut self,
        mint: Pubkey,
    ) -> AnchorResult<(AccountPositionKey, Option<&mut AccountPosition>)> {
        // check for an existing position
        if let Some(p) = self.map.iter().find(|p| p.mint == mint) {
            return Ok((*p, None));
        }

        // find the first free space to store the position info
        let (index, free_position) = self
            .positions
            .iter_mut()
            .enumerate()
            .find(|(_, p)| p.token == Pubkey::default())
            .ok_or_else(|| error!(ErrorCode::MaxPositions))?;

        // add the new entry to the sorted map
        let key = AccountPositionKey {
            mint,
            index: index as u64,
        };

        let max_index = usize::try_from(self.length).unwrap();
        self.map[max_index] = key;
        self.map[..max_index + 1].sort_by_key(|p| p.mint);
        self.length += 1;

        // mark position as not free
        free_position.token = mint;

        // return the allocated position to be initialized further
        Ok((key, Some(free_position)))
    }

    /// Remove a position from the margin account.
    ///
    /// # Error
    ///
    /// - If an account with the `mint` does not exist.
    /// - If the position's address is not the same as the `account`
    pub fn remove(&mut self, mint: &Pubkey, account: &Pubkey) -> AnchorResult<AccountPosition> {
        // Return Error if position is not registered
        let map_index = self
            .get_map_index(mint)
            .ok_or(ErrorCode::PositionNotRegistered)?;

        // Get the map whose position to remove
        let map: AccountPositionKey = self.map[map_index];
        let position_index = usize::try_from(map.index).unwrap();

        // Take a copy of the position to be removed
        let position = self.positions[position_index];

        // Check that the position is correct
        // Verify that the registered token account for the magin account matches the token account provided
        if &position.address != account {
            return err!(ErrorCode::PositionNotRegistered);
        }

        // Remove the position
        self.positions[position_index] = Zeroable::zeroed();

        // Move the map elements up by 1 to replace map position being removed
        let length = usize::try_from(self.length).unwrap();
        self.map.copy_within(map_index + 1..length, map_index);

        // Clear the map at the last slot of the array, as it is shifted up
        self.map[length - 1].mint = Pubkey::default();
        self.map[length - 1].index = 0;
        self.length -= 1;

        Ok(position)
    }

    pub fn get(&self, mint: &Pubkey) -> Option<&AccountPosition> {
        let key = self.get_key(mint)?;
        let position = &self.positions[usize::try_from(key.index).unwrap()];

        Some(position)
    }

    pub fn get_mut(&mut self, mint: &Pubkey) -> Option<&mut AccountPosition> {
        let key = self.get_key(mint)?;
        let position = &mut self.positions[usize::try_from(key.index).unwrap()];

        Some(position)
    }

    pub fn get_key(&self, mint: &Pubkey) -> Option<&AccountPositionKey> {
        Some(&self.map[self.get_map_index(mint)?])
    }

    fn get_map_index(&self, mint: &Pubkey) -> Option<usize> {
        self.map[..usize::try_from(self.length).unwrap()]
            .binary_search_by_key(mint, |p| p.mint)
            .ok()
    }
}

/// Data necessary to register a position
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct PositionConfigUpdate {
    /// The token mint for the position token
    pub mint: Pubkey,

    /// The token program of the token mint
    pub token_program: Pubkey,

    /// The number of decimals for the position token
    pub decimals: u8,

    /// The address of the account holding the tokens
    pub address: Pubkey,

    /// The airspace this position token belongs to
    pub airspace: Pubkey,

    /// The adapter registering this position
    pub adapter: Pubkey,

    /// Description of the position token
    pub kind: TokenKind,

    /// A weight on the value of this asset when counting collateral
    pub value_modifier: u16,

    /// Max staleness in seconds for the position balance
    pub max_staleness: u64,

    /// Token features
    pub token_features: TokenFeatures,
}

impl PositionConfigUpdate {
    /// Generate the configuration from `TokenConfig` account data
    pub fn new_from_config(
        config: &Account<TokenConfig>,
        mint_decimals: u8,
        address: Pubkey,
        adapter: Pubkey,
    ) -> anchor_lang::Result<Self> {
        let token_program = match config.token_kind {
            TokenKind::Collateral => config.underlying_mint_token_program,
            TokenKind::Claim => config.mint_token_program,
            TokenKind::AdapterCollateral => config.mint_token_program,
        };
        // Check that the token program is a supported token program
        if !anchor_spl::token_interface::TokenAccount::owners().contains(&token_program) {
            msg!("Unsupported token program: {}", token_program);
            return err!(ErrorCode::UnknownTokenProgram);
        }
        Ok(Self {
            mint: config.mint,
            token_program,
            decimals: mint_decimals,
            address,
            airspace: config.airspace,
            adapter,
            kind: config.token_kind,
            value_modifier: config.value_modifier,
            max_staleness: config.max_staleness,
            token_features: config.token_features,
        })
    }
}
