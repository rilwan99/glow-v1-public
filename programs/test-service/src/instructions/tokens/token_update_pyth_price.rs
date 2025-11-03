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
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;

use crate::state::TokenInfo;

#[derive(Accounts)]
pub struct TokenUpdatePythPrice<'info> {
    oracle_authority: Signer<'info>,

    info: Account<'info, TokenInfo>,

    /// The pyth price is validated with the feed ID
    #[account(mut)]
    price_update: AccountInfo<'info>,
}

pub fn token_update_pyth_price_handler(
    ctx: Context<TokenUpdatePythPrice>,
    feed_id: [u8; 32],
    price: i64,
    conf: i64,
    expo: i32,
) -> Result<()> {
    // TODO: do we want to check the oracle authority, or does it not matter for testing?
    let mut data = ctx.accounts.price_update.try_borrow_mut_data()?;
    let mut price_update = PriceUpdateV2::try_deserialize(&mut &data[..])?;
    let clock = Clock::get()?;

    // This is fine as we use it only for testing
    price_update.get_price_unchecked(&feed_id)?;

    assert!(price > 0);

    price_update.price_message.prev_publish_time = price_update.price_message.publish_time;
    price_update.price_message.price = price;
    price_update.price_message.exponent = expo;
    price_update.price_message.conf = conf as u64;
    price_update.price_message.publish_time = clock.unix_timestamp;
    price_update.price_message.ema_price = price;
    price_update.price_message.ema_conf = conf as u64;

    // Write back
    price_update.serialize(&mut &mut data[8..])?;

    Ok(())
}
