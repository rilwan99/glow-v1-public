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

// Allow this until fixed upstream
#![allow(clippy::result_large_err)]

use anchor_lang::prelude::*;

pub mod error;
pub mod state;

mod instructions;
mod util;

use instructions::*;

pub use instructions::TokenCreateParams;

declare_id!("test7JXXboKpc8hGTadvoXcFWN4xgnHLGANU92JKrwA");

pub mod seeds {
    use super::*;

    #[constant]
    pub const TOKEN_MINT: &[u8] = b"token-mint";

    #[constant]
    pub const TOKEN_INFO: &[u8] = b"token-info";

    #[constant]
    pub const TOKEN_ACCOUNT: &[u8] = b"token-account";

    #[constant]
    pub const TEST_SERVICE_AUTHORITY: &[u8] = b"test-service-authority";
}

#[program]
pub mod test_service {
    use super::*;

    /// Create a token mint based on some seed
    ///
    /// The created mint has a this program as the authority, any user may request
    /// tokens via the `token_request` instruction up to the limit specified in the
    /// `max_amount` parameter.
    ///
    /// This will also create pyth oracle accounts for the token.
    pub fn token_create(ctx: Context<TokenCreate>, params: TokenCreateParams) -> Result<()> {
        token_create_handler(ctx, params)
    }

    /// Same as token_create except it does not create the mint. The mint should
    /// be created some other way, such as by an adapter.
    pub fn token_register(ctx: Context<TokenRegister>, params: TokenCreateParams) -> Result<()> {
        token_register_handler(ctx, params)
    }

    /// Initialize the token info and oracles for the native token mint
    ///
    /// Since the native mint is a special case that can't be owned by this program,
    /// this instruction allows creating an oracle for it.
    pub fn token_init_native(
        ctx: Context<TokenInitNative>,
        feed_id: [u8; 32],
        oracle_authority: Pubkey,
    ) -> Result<()> {
        token_init_native_handler(ctx, feed_id, oracle_authority)
    }

    /// Request tokens be minted by the faucet.
    pub fn token_request(ctx: Context<TokenRequest>, amount: u64) -> Result<()> {
        token_request_handler(ctx, amount)
    }

    /// Relinquish the authority of the token mint to a new authority.
    pub fn token_relinquish_authority(ctx: Context<TokenRelinquishAuthority>) -> Result<()> {
        token_relinquish_authority_handler(ctx)
    }

    /// Update the pyth oracle price account for a token.
    pub fn token_update_pyth_price(
        ctx: Context<TokenUpdatePythPrice>,
        feed_id: [u8; 32],
        price: i64,
        conf: i64,
        expo: i32,
    ) -> Result<()> {
        token_update_pyth_price_handler(ctx, feed_id, price, conf, expo)
    }

    /// Invokes arbitrary program iff an account is not yet initialized.
    /// Typically used to run an instruction that initializes the account,
    /// ensuring multiple initializations will not collide.
    pub fn if_not_initialized(ctx: Context<IfNotInitialized>, instruction: Vec<u8>) -> Result<()> {
        if_not_initialized_handler(ctx, instruction)
    }

    /// Initialize a slippy pool
    pub fn init_slippy_pool(ctx: Context<InitSlippyPool>) -> Result<()> {
        init_slippy_pool_handler(ctx)
    }

    /// Swap in the slippy pool
    pub fn swap_slippy_pool(
        ctx: Context<SwapSlippyPool>,
        amount_in: u64,
        a_to_b: bool,
        a_to_b_exchange_rate: f64,
        slippage: f64, // in percentage
    ) -> Result<()> {
        swap_slippy_pool_handler(ctx, amount_in, a_to_b, a_to_b_exchange_rate, slippage)
    }

    pub fn init_test_service_authority(ctx: Context<InitTestServiceAuthority>) -> Result<()> {
        init_test_service_authority_handler(ctx)
    }

    pub fn register_adapter_position(ctx: Context<RegisterAdapterPosition>) -> Result<()> {
        register_adapter_position_handler(ctx)
    }

    pub fn close_adapter_position(ctx: Context<CloseAdapterPosition>) -> Result<()> {
        close_adapter_position_handler(ctx)
    }
}
