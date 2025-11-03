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
use anchor_spl::token::{spl_token::native_mint::ID as NATIVE_MINT_ID, Mint, Token};
use glow_program_common::oracle::pyth_feed_ids::sol_usd;
use pyth_solana_receiver_sdk::price_update::{PriceFeedMessage, PriceUpdateV2, VerificationLevel};

use crate::{seeds::TOKEN_INFO, state::TokenInfo};

#[derive(Accounts)]
#[instruction(feed_id: [u8; 32])]
pub struct TokenInitNative<'info> {
    #[account(mut)]
    payer: Signer<'info>,

    #[account(address = NATIVE_MINT_ID)]
    mint: Box<Account<'info, Mint>>,

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
                &[0u8, 0],
                feed_id.as_ref(),
              ],
              bump,
              space = 8 + std::mem::size_of::<PriceUpdateV2>(),
              payer = payer,
    )]
    price_update: AccountInfo<'info>,

    token_program: Program<'info, Token>,
    system_program: Program<'info, System>,
    rent: Sysvar<'info, Rent>,
}

pub fn token_init_native_handler(
    ctx: Context<TokenInitNative>,
    feed_id: [u8; 32],
    oracle_authority: Pubkey,
) -> Result<()> {
    let info = &mut ctx.accounts.info;

    info.name = "SOL".to_string();
    info.symbol = "SOL".to_string();
    info.mint = ctx.accounts.mint.key();
    info.authority = Pubkey::default();
    info.pyth_feed_id = sol_usd();
    require!(
        info.pyth_feed_id == feed_id,
        crate::error::TestServiceError::InvalidSeeds
    );
    info.oracle_authority = oracle_authority;
    info.max_request_amount = 0;
    info.source_symbol = "SOL".to_string();
    info.price_ratio = 1.0;

    let clock = Clock::get()?;

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
    // Manually initialize price_update
    let mut data = ctx.accounts.price_update.try_borrow_mut_data()?;
    data[0..8].copy_from_slice(&PriceUpdateV2::discriminator());
    price_update.serialize(&mut &mut data[8..])?;

    Ok(())
}
