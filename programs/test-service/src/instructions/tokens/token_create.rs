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

use anchor_lang::{prelude::*, Discriminator};
use anchor_spl::token_interface::{Mint, TokenInterface};
use glow_program_common::oracle::TokenPriceOracle;
use pyth_solana_receiver_sdk::price_update::{PriceFeedMessage, PriceUpdateV2, VerificationLevel};

use crate::{
    seeds::{TOKEN_INFO, TOKEN_MINT},
    state::TokenInfo,
};

#[derive(AnchorDeserialize, AnchorSerialize, Debug, Clone)]
pub struct TokenCreateParams {
    /// The symbol string for the token
    pub symbol: String,

    /// The name or description of the token
    ///
    /// Used to derive the mint address
    pub name: String,

    /// The decimals for the mint
    pub decimals: u8,

    /// The authority over the token
    pub authority: Pubkey,

    /// The authority to set prices
    pub oracle_authority: Pubkey,

    /// The maximum amount of the token a user can request to mint in a
    /// single instruction.
    pub max_amount: u64,

    /// the symbol of the mainnet product from which the price will be derived
    pub source_symbol: String,

    /// multiplied by the mainnet price to get the price of this asset
    pub price_ratio: f64,

    /// Oracle details by different oracles
    pub price_oracle: TokenPriceOracle,
}

#[derive(Accounts)]
#[instruction(params: TokenCreateParams)]
pub struct TokenCreate<'info> {
    #[account(mut)]
    payer: Signer<'info>,

    #[account(init,
              seeds = [
                TOKEN_MINT,
                params.name.as_bytes()
              ],
              bump,
              mint::decimals = params.decimals,
              mint::authority = info,
              mint::freeze_authority = info,
              mint::token_program = token_program,
              payer = payer)]
    mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(init,
              seeds = [
                TOKEN_INFO,
                mint.key().as_ref()
              ],
              bump,
              space = TokenInfo::SIZE,
              payer = payer
    )]
    info: Box<Account<'info, TokenInfo>>,

    #[account(init_if_needed,
              seeds = [
                [0u8, 0].as_ref(),
                params.price_oracle.pyth_feed_id().unwrap().as_ref(),
              ],
              bump,
              space = 8 + std::mem::size_of::<PriceUpdateV2>(),
              payer = payer,
    )]
    price_update: AccountInfo<'info>,

    token_program: Interface<'info, TokenInterface>,
    system_program: Program<'info, System>,
    rent: Sysvar<'info, Rent>,
}

pub fn token_create_handler(ctx: Context<TokenCreate>, params: TokenCreateParams) -> Result<()> {
    let clock = Clock::get()?;

    let info = &mut ctx.accounts.info;

    info.bump_seed = ctx.bumps.info;
    info.symbol.clone_from(&params.symbol);
    info.name = params.name;
    info.mint = ctx.accounts.mint.key();
    info.authority = params.authority;
    info.pyth_feed_id = *params.price_oracle.pyth_feed_id().unwrap();
    info.oracle_authority = params.oracle_authority;
    info.max_request_amount = params.max_amount;
    info.source_symbol.clone_from(&params.source_symbol);
    info.price_ratio = params.price_ratio;

    let price_update = PriceUpdateV2 {
        write_authority: info.oracle_authority,
        verification_level: VerificationLevel::Full,
        price_message: PriceFeedMessage {
            feed_id: info.pyth_feed_id,
            price: 0,
            conf: 0,
            exponent: -8,
            publish_time: clock.unix_timestamp,
            prev_publish_time: 0,
            ema_price: 0,
            ema_conf: 0,
        },
        posted_slot: clock.slot,
    };
    let mut data = ctx.accounts.price_update.try_borrow_mut_data()?;
    data[0..8].copy_from_slice(&PriceUpdateV2::discriminator());
    price_update.serialize(&mut &mut data[8..])?;

    Ok(())
}
