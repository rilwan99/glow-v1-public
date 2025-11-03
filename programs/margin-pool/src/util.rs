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

use crate::ErrorCode;
use anchor_lang::{prelude::AccountInfo, solana_program::clock::UnixTimestamp, Result};
use anchor_spl::token::spl_token;
use anchor_spl::token_2022::spl_token_2022::extension::transfer_fee::TransferFeeConfig;
use anchor_spl::token_2022::spl_token_2022::{
    extension::{BaseStateWithExtensions, ExtensionType, StateWithExtensions},
    state::Mint as Mint2022,
};
use anchor_spl::token_interface::spl_pod::bytemuck::pod_from_bytes;
use glow_program_common::Number;
use solana_program::program_pack::Pack;

pub const SECONDS_PER_HOUR: UnixTimestamp = 3600;
pub const SECONDS_PER_2H: UnixTimestamp = SECONDS_PER_HOUR * 2;
pub const SECONDS_PER_12H: UnixTimestamp = SECONDS_PER_HOUR * 12;
pub const SECONDS_PER_DAY: UnixTimestamp = SECONDS_PER_HOUR * 24;
pub const SECONDS_PER_WEEK: UnixTimestamp = SECONDS_PER_DAY * 7;
pub const SECONDS_PER_YEAR: UnixTimestamp = 31_536_000;
pub const MAX_ACCRUAL_SECONDS: UnixTimestamp = SECONDS_PER_WEEK;

static_assertions::const_assert_eq!(SECONDS_PER_HOUR, 60 * 60);
static_assertions::const_assert_eq!(SECONDS_PER_2H, 60 * 60 * 2);
static_assertions::const_assert_eq!(SECONDS_PER_12H, 60 * 60 * 12);
static_assertions::const_assert_eq!(SECONDS_PER_DAY, 60 * 60 * 24);
static_assertions::const_assert_eq!(SECONDS_PER_WEEK, 60 * 60 * 24 * 7);
static_assertions::const_assert_eq!(SECONDS_PER_YEAR, 60 * 60 * 24 * 365);

/// Computes the effective applicable interest rate assuming continuous
/// compounding for the given number of slots.
///
/// Uses an approximation calibrated for accuracy to twenty decimals places,
/// though the current configuration of Number does not support that.
pub fn compound_interest(rate: Number, seconds: UnixTimestamp) -> Number {
    // The two panics below are implementation details, chosen to facilitate convenient
    // implementation of compounding. They can be relaxed with a bit of additional work.
    // The "seconds" guards are chosen to guarantee accuracy under the assumption that
    // the rate is not more than one.

    if rate > Number::ONE * 2 {
        panic!("Not implemented; interest rate too large for compound_interest()");
    }

    let terms = match seconds {
        _ if seconds <= SECONDS_PER_2H => 5,
        _ if seconds <= SECONDS_PER_12H => 6,
        _ if seconds <= SECONDS_PER_DAY => 7,
        _ if seconds <= SECONDS_PER_WEEK => 10,
        _ => panic!("Not implemented; too many seconds in compound_interest()"),
    };

    let x = rate * seconds / SECONDS_PER_YEAR;

    glow_program_common::expm1_approx(x, terms)
}

/// Linear interpolation between (x0, y0) and (x1, y1).
pub fn interpolate(x: Number, x0: Number, x1: Number, y0: Number, y1: Number) -> Number {
    assert!(x >= x0);
    assert!(x <= x1);

    y0 + ((x - x0) * (y1 - y0)) / (x1 - x0)
}

// These are the disabled mint extensions as part of the RFC Supporting Token 2022 Program.
static DISABLED_MINT_EXTENSIONS: &[&ExtensionType] = &[
    &ExtensionType::DefaultAccountState,
    &ExtensionType::NonTransferable,
    &ExtensionType::NonTransferableAccount,
    &ExtensionType::InterestBearingConfig,
    // AUSD uses TransferFeeConfig as seen here:
    // https://solscan.io/token/AUSD1jCcCyPLybk1YnvPWsHQSrZ46dxwoMniN4N2UEB9#extensions
    // If the config is disabled, the amount is not applicable anymore.
    // Conversely, if we want to use it, we need to comment both out.
    // DO NOT UNCOMMENT BELOW.
    // &ExtensionType::TransferFeeConfig,
    // &ExtensionType::TransferFeeAmount,
    //
    // PYUSD is mentioned to use it here:
    // - https://pyusd.mirror.xyz/TpEwPNybrwzPSSQenLtO4kggy98KH4oQRc06ggVnA0k
    // - https://kauri.finance/academy/building-with-paypal-usd
    // But is not listed in the whitepaper below.
    // DO NOT UNCOMMENT BELOW FOR NOW.
    // &ExtensionType::MemoTransfer,
    //
    // PYUSD whitepaper mentions it:
    // https://www.paypalobjects.com/devdoc/community/PYUSD-Solana-White-Paper.pdf
    // DO NOT UNCOMMENT BELOW FOR NOW.
    // &ExtensionType::PermanentDelegate,
];

pub fn validate_mint_extension(mint: AccountInfo<'_>) -> Result<()> {
    let data = mint.try_borrow_data()?;

    // If no extensions available, we bail early
    if data.len() == spl_token::state::Mint::LEN {
        return Ok(());
    }

    let state = StateWithExtensions::<Mint2022>::unpack(&data)?;
    let extensions = state.get_extension_types()?;

    // If the mint token contains either of the disabled extensions, we error out early
    if extensions
        .iter()
        .any(|ext| DISABLED_MINT_EXTENSIONS.contains(&ext))
    {
        return Err(ErrorCode::TokenExtensionNotEnabled.into());
    }

    // If the mint token contains TransferFeeConfig AND has fees enabled
    validate_zero_transfer_fee(state)?;

    Ok(())
}

pub fn validate_zero_transfer_fee<'info>(state: StateWithExtensions<Mint2022>) -> Result<()> {
    if let Ok(bytes) = state.get_extension_bytes::<TransferFeeConfig>() {
        let config = pod_from_bytes::<TransferFeeConfig>(bytes)?;
        if u16::from(config.older_transfer_fee.transfer_fee_basis_points) > 0
            || u16::from(config.newer_transfer_fee.transfer_fee_basis_points) > 0
        {
            return Err(ErrorCode::TokenExtensionNotEnabled.into());
        }
    }

    Ok(())
}
