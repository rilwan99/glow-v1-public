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
use anchor_spl::token_interface::TokenInterface;
use pyth_solana_receiver_sdk::price_update::{PriceFeedMessage, PriceUpdateV2, VerificationLevel};

use crate::{seeds::TOKEN_INFO, state::TokenInfo, TokenCreateParams};

#[derive(Accounts)]
#[instruction(params: TokenCreateParams)]
pub struct TokenRegister<'info> {
    #[account(mut)]
    payer: Signer<'info>,

    mint: AccountInfo<'info>,

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

    #[account(init,
              seeds = [
                &[0u8, 0],
                params.price_oracle.pyth_feed_id().unwrap().as_ref(),
              ],
              bump,
              space = 8 + std::mem::size_of::<PriceUpdateV2>(),
              payer = payer,
            //   owner = crate::ID,
    )]
    price_update: AccountInfo<'info>,

    token_program: Interface<'info, TokenInterface>,
    system_program: Program<'info, System>,
    rent: Sysvar<'info, Rent>,
}

pub fn token_register_handler(
    ctx: Context<TokenRegister>,
    params: TokenCreateParams,
) -> Result<()> {
    let info = &mut ctx.accounts.info;

    info.bump_seed = ctx.bumps.info;
    info.symbol = params.symbol.clone();
    info.name = params.name;
    info.mint = ctx.accounts.mint.key();
    info.authority = params.authority;
    info.pyth_feed_id = *params.price_oracle.pyth_feed_id().unwrap();
    info.oracle_authority = params.oracle_authority;
    info.max_request_amount = params.max_amount;
    info.source_symbol = params.source_symbol.clone();
    info.price_ratio = params.price_ratio;

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
    let mut data = ctx.accounts.price_update.try_borrow_mut_data()?;
    data[0..8].copy_from_slice(&PriceUpdateV2::discriminator());
    price_update.serialize(&mut &mut data[8..])?;

    Ok(())
}
