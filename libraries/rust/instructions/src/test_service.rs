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

use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::token::ID as TOKEN_ID;
use glow_program_common::oracle::pyth_feed_ids::sol_usd;
use solana_sdk::{
    instruction::Instruction, pubkey, pubkey::Pubkey, rent::Rent, system_program, sysvar::SysvarId,
};

use glow_test_service::seeds::{TOKEN_INFO, TOKEN_MINT};

pub use glow_test_service::TokenCreateParams;

pub use glow_test_service::ID as TEST_SERVICE_PROGRAM;

use crate::derive_pyth_price_feed_account;
use crate::MintInfo;

/// Get instruction to create a token as described
pub fn token_create(
    payer: &Pubkey,
    params: &TokenCreateParams,
    token_program: Pubkey,
) -> Instruction {
    let mint = derive_token_mint(&params.name);

    let accounts = glow_test_service::accounts::TokenCreate {
        payer: *payer,
        mint,
        info: derive_token_info(&mint),
        token_program,
        system_program: system_program::ID,
        rent: Rent::id(),
        price_update: derive_pyth_price_feed_account(
            params.price_oracle.pyth_feed_id().unwrap(),
            None,
            glow_test_service::ID,
        ),
    }
    .to_account_metas(None);

    Instruction {
        program_id: glow_test_service::ID,
        accounts,
        data: glow_test_service::instruction::TokenCreate {
            params: params.clone(),
        }
        .data(),
    }
}

/// Get instruction to register a token as described
pub fn token_register(payer: &Pubkey, mint: MintInfo, params: &TokenCreateParams) -> Instruction {
    let accounts = glow_test_service::accounts::TokenRegister {
        payer: *payer,
        mint: mint.address,
        info: derive_token_info(&mint.address),
        price_update: derive_pyth_price_feed_account(
            params.price_oracle.pyth_feed_id().unwrap(),
            None,
            glow_test_service::ID,
        ),
        token_program: mint.token_program(),
        system_program: system_program::ID,
        rent: Rent::id(),
    }
    .to_account_metas(None);

    Instruction {
        program_id: glow_test_service::ID,
        accounts,
        data: glow_test_service::instruction::TokenRegister {
            params: params.clone(),
        }
        .data(),
    }
}

/// Get instruction to initialize native token
pub fn token_init_native(payer: &Pubkey, oracle_authority: &Pubkey) -> Instruction {
    let mint = anchor_spl::token::spl_token::native_mint::ID;

    let accounts = glow_test_service::accounts::TokenInitNative {
        payer: *payer,
        mint,
        info: derive_token_info(&mint),
        price_update: derive_pyth_price_feed_account(&sol_usd(), None, glow_test_service::ID),
        token_program: TOKEN_ID,
        system_program: system_program::ID,
        rent: Rent::id(),
    }
    .to_account_metas(None);

    Instruction {
        program_id: glow_test_service::ID,
        accounts,
        data: glow_test_service::instruction::TokenInitNative {
            feed_id: sol_usd(),
            oracle_authority: *oracle_authority,
        }
        .data(),
    }
}

/// Request a number of tokens be minted
pub fn token_request(
    payer: &Pubkey,
    requester: &Pubkey,
    mint: MintInfo,
    destination: &Pubkey,
    amount: u64,
) -> Instruction {
    let accounts = glow_test_service::accounts::TokenRequest {
        payer: *payer,
        requester: *requester,
        mint: mint.address,
        info: derive_token_info(&mint.address),
        destination: *destination,
        token_program: mint.token_program(),
    }
    .to_account_metas(None);

    Instruction {
        program_id: glow_test_service::ID,
        accounts,
        data: glow_test_service::instruction::TokenRequest { amount }.data(),
    }
}

/// Request a number of tokens be minted
pub fn token_relinquish_authority(
    payer: &Pubkey,
    mint: MintInfo,
    new_authority: &Pubkey,
) -> Instruction {
    let accounts = glow_test_service::accounts::TokenRelinquishAuthority {
        payer: *payer,
        mint: mint.address,
        info: derive_token_info(&mint.address),
        new_authority: *new_authority,
        token_program: mint.token_program(),
    }
    .to_account_metas(None);

    Instruction {
        program_id: glow_test_service::ID,
        accounts,
        data: glow_test_service::instruction::TokenRelinquishAuthority {}.data(),
    }
}

/// Update the pyth price for a token
pub fn token_update_pyth_price(
    authority: &Pubkey,
    mint: &Pubkey,
    feed_id: [u8; 32],
    price: i64,
    conf: i64,
    expo: i32,
) -> Instruction {
    assert_ne!(feed_id, [0; 32], "Please don't use a zeroed out feed id");
    let accounts = glow_test_service::accounts::TokenUpdatePythPrice {
        oracle_authority: *authority,
        info: derive_token_info(mint),
        price_update: derive_pyth_price_feed_account(&feed_id, None, glow_test_service::ID),
    }
    .to_account_metas(None);

    Instruction {
        program_id: glow_test_service::ID,
        accounts,
        data: glow_test_service::instruction::TokenUpdatePythPrice {
            feed_id,
            price,
            conf,
            expo,
        }
        .data(),
    }
}

/// if the account is not initialized, invoke the instruction
pub fn if_not_initialized(account_to_check: Pubkey, ix: Instruction) -> Instruction {
    let mut accounts = glow_test_service::accounts::IfNotInitialized {
        program: ix.program_id,
        account_to_check,
    }
    .to_account_metas(None);

    accounts.extend(ix.accounts);

    Instruction {
        accounts,
        program_id: glow_test_service::ID,
        data: glow_test_service::instruction::IfNotInitialized {
            instruction: ix.data,
        }
        .data(),
    }
}

pub fn init_slippy_pool(
    mint_a: MintInfo,
    mint_b: MintInfo,
    payer: Pubkey,
) -> (Pubkey, Instruction) {
    let slippy = Pubkey::find_program_address(
        &[
            mint_a.address.as_ref(),
            mint_b.address.as_ref(),
            b"slippy-pool",
        ],
        &glow_test_service::ID,
    )
    .0;

    (
        slippy,
        Instruction {
            program_id: glow_test_service::ID,
            accounts: glow_test_service::accounts::InitSlippyPool {
                payer,
                slippy,
                mint_a: mint_a.address,
                mint_b: mint_b.address,
                vault_a: Pubkey::find_program_address(
                    &[slippy.as_ref(), mint_a.address.as_ref()],
                    &glow_test_service::ID,
                )
                .0,
                vault_b: Pubkey::find_program_address(
                    &[slippy.as_ref(), mint_b.address.as_ref()],
                    &glow_test_service::ID,
                )
                .0,
                token_program_a: mint_a.token_program(),
                token_program_b: mint_b.token_program(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: glow_test_service::instruction::InitSlippyPool {}.data(),
        },
    )
}

pub fn swap_slippy_pool(
    mint_a: MintInfo,
    mint_b: MintInfo,
    trader: Pubkey,
    amount_in: u64,
    a_to_b: bool,
    a_to_b_exchange_rate: f64,
    slippage: f64,
) -> Instruction {
    let slippy = Pubkey::find_program_address(
        &[
            mint_a.address.as_ref(),
            mint_b.address.as_ref(),
            b"slippy-pool",
        ],
        &glow_test_service::ID,
    )
    .0;

    Instruction {
        program_id: glow_test_service::ID,
        accounts: glow_test_service::accounts::SwapSlippyPool {
            signer: trader,
            slippy,
            mint_a: mint_a.address,
            mint_b: mint_b.address,
            vault_a: Pubkey::find_program_address(
                &[slippy.as_ref(), mint_a.address.as_ref()],
                &glow_test_service::ID,
            )
            .0,
            vault_b: Pubkey::find_program_address(
                &[slippy.as_ref(), mint_b.address.as_ref()],
                &glow_test_service::ID,
            )
            .0,
            token_program_a: mint_a.token_program(),
            token_program_b: mint_b.token_program(),
            signer_token_a: mint_a.associated_token_address(&trader),
            signer_token_b: mint_b.associated_token_address(&trader),
        }
        .to_account_metas(None),
        data: glow_test_service::instruction::SwapSlippyPool {
            amount_in,
            a_to_b,
            a_to_b_exchange_rate,
            slippage,
        }
        .data(),
    }
}

/// Get the token mint address for a given token name
pub fn derive_token_mint(name: &str) -> Pubkey {
    if name == "SOL" || name == "Solana" {
        return pubkey!("So11111111111111111111111111111111111111112");
    }

    Pubkey::find_program_address(&[TOKEN_MINT, name.as_bytes()], &glow_test_service::ID).0
}

/// Get the token info account
pub fn derive_token_info(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[TOKEN_INFO, mint.as_ref()], &glow_test_service::ID).0
}
