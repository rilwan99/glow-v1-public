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

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::Duration;

use anchor_lang::{prelude::*, Discriminator};
use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token_2022::spl_token_2022;
use anchor_spl::token_2022::spl_token_2022::state::Mint;
use anchor_spl::{token::ID as TOKEN_ID, token_2022::ID as TOKEN_2022_ID};
use anyhow::{bail, Context, Error};
use bytemuck::Zeroable;

use anchor_spl::token::{
    spl_token::{
        self,
        instruction::{initialize_mint, mint_to},
    },
    TokenAccount,
};

use glow_instructions::airspace::AirspaceDetails;
use glow_instructions::test_service::derive_token_mint;
use glow_instructions::{derive_pyth_price_feed_account, MintInfo};
use glow_margin_sdk::ix_builder::test_service::{self, derive_token_info};
use glow_margin_sdk::solana::transaction::{
    SendTransactionBuilder, TransactionBuilder, TransactionBuilderExt,
};
use glow_margin_sdk::tokens::TokenPrice;
use glow_margin_sdk::util::asynchronous::with_retries_and_timeout;
use glow_program_common::oracle::TokenPriceOracle;
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use glow_solana_client::transaction::{ToTransaction, WithSigner};
use glow_test_service::seeds::TOKEN_MINT;
use glow_test_service::TokenCreateParams;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solana_program_test::ProgramTestContext;
use solana_sdk::account::ReadableAccount;
use solana_sdk::instruction::Instruction;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::{system_instruction, system_program};
use solana_sdk::{system_instruction::create_account, transaction::Transaction};

use glow_program_common::Number128;
use glow_simulation::Keygen;
use glow_simulation::{runtime::TestRuntimeRpcClient, DeterministicKeygen};

use crate::send_and_confirm;

pub mod preset_token_configs {

    use glow_program_common::oracle::pyth_feed_ids::*;

    use super::*;

    pub fn usdc_config(authority: Pubkey) -> TokenCreateParams {
        TokenCreateParams {
            symbol: "USDC".to_string(),
            name: "USD Coin".to_string(),
            decimals: 6,
            authority,
            oracle_authority: authority,
            max_amount: u64::MAX,
            source_symbol: "USDC".to_string(),
            price_ratio: 1.0,
            price_oracle: TokenPriceOracle::PythPull {
                feed_id: usdc_usd(),
            },
        }
    }

    pub fn usdt_config(authority: Pubkey) -> TokenCreateParams {
        TokenCreateParams {
            symbol: "USDT".to_string(),
            name: "Tether".to_string(),
            decimals: 6,
            authority,
            oracle_authority: authority,
            max_amount: u64::MAX,
            source_symbol: "USDT".to_string(),
            price_ratio: 1.0,
            price_oracle: TokenPriceOracle::PythPull {
                feed_id: usdt_usd(),
            },
        }
    }

    pub fn tsol_config(authority: Pubkey) -> TokenCreateParams {
        TokenCreateParams {
            symbol: "TSOL".to_string(),
            name: "Test SOL".to_string(),
            decimals: 9,
            authority,
            oracle_authority: authority,
            max_amount: u64::MAX,
            source_symbol: "SOL".to_string(),
            price_ratio: 1.0,
            price_oracle: TokenPriceOracle::PythPull { feed_id: sol_usd() },
        }
    }

    pub fn gsol_config(authority: Pubkey) -> TokenCreateParams {
        TokenCreateParams {
            symbol: "GSOL".to_string(),
            name: "Glow SOL".to_string(),
            decimals: 9,
            authority,
            oracle_authority: authority,
            max_amount: u64::MAX,
            source_symbol: "GSOL".to_string(),
            price_ratio: 1.0,
            price_oracle: TokenPriceOracle::PythPullRedemption {
                feed_id: gsol_sol_rr(),
                quote_feed_id: sol_usd(),
            },
        }
    }

    pub fn ssol_config(authority: Pubkey) -> TokenCreateParams {
        TokenCreateParams {
            symbol: "SSOL".to_string(),
            name: "Test Staked SOL".to_string(),
            decimals: 9,
            authority,
            oracle_authority: authority,
            max_amount: u64::MAX,
            source_symbol: "SSOL".to_string(),
            price_ratio: 1.0,
            price_oracle: TokenPriceOracle::PythPullRedemption {
                feed_id: ssol_sol_rr(),
                quote_feed_id: sol_usd(),
            },
        }
    }

    pub fn btc_config(authority: Pubkey) -> TokenCreateParams {
        TokenCreateParams {
            symbol: "BTC".to_string(),
            name: "Bitcoin".to_string(),
            decimals: 8,
            authority,
            oracle_authority: authority,
            max_amount: u64::MAX,
            source_symbol: "BTC".to_string(),
            price_ratio: 1.0,
            price_oracle: TokenPriceOracle::PythPull { feed_id: btc_usd() },
        }
    }
}

#[inline]
pub const fn get_token_program(is_token_2022: bool) -> Pubkey {
    if is_token_2022 {
        TOKEN_2022_ID
    } else {
        TOKEN_ID
    }
}

/// Utility for managing the creation of tokens and their prices
/// in some kind of testing environment
#[derive(Clone)]
pub struct TokenManager {
    pub ctx: TestRuntimeRpcClient,
}

impl TokenManager {
    pub fn new(ctx: TestRuntimeRpcClient) -> Self {
        Self { ctx }
    }

    /// Create a new token mint, with optional mint and freeze authorities.
    ///
    /// # Params
    ///
    /// `decimals` - the number of decimal places the mint should have
    /// `mint_authority` - optional authority to mint tokens, defaults to the payer
    /// `freeze_authority` - optional authority to freeze tokens, has no default
    pub async fn create_token(
        &self,
        decimals: u8,
        mint_authority: Option<&Pubkey>,
        freeze_authority: Option<&Pubkey>,
        is_token_2022: bool,
    ) -> std::result::Result<MintInfo, Error> {
        let keypair = self.ctx.keygen.generate_key();
        self.create_token_from(
            keypair,
            decimals,
            mint_authority,
            freeze_authority,
            is_token_2022,
        )
        .await
    }

    pub async fn create_associated_token(
        &self,
        mint: MintInfo,
        owner: &Pubkey,
    ) -> std::result::Result<Pubkey, Error> {
        let payer = self.ctx.payer();
        mint.create_associated_token_account_idempotent(owner, &payer.pubkey())
            .with_signer(payer)
            .send_and_confirm(&self.ctx.rpc())
            .await?;
        Ok(mint.associated_token_address(owner))
    }

    /// Create a token with a Pyth pull oracle
    pub async fn create_token_v2(
        &self,
        params: &TokenCreateParams,
        initial_price: i64,
        is_token_2022: bool,
    ) -> std::result::Result<(MintInfo, TokenPriceOracle), Error> {
        let payer = self.ctx.payer();
        let token_program = get_token_program(is_token_2022);

        let ix_token_create = test_service::token_create(&payer.pubkey(), params, token_program);
        let mint = derive_token_mint(&params.name);
        let ix_pyth_update_price = test_service::token_update_pyth_price(
            &payer.pubkey(),
            &mint,
            *params.price_oracle.pyth_feed_id().unwrap(),
            initial_price,
            100,
            -8,
        );

        let tx = Transaction::new_signed_with_payer(
            &[ix_token_create, ix_pyth_update_price],
            Some(&payer.pubkey()),
            &[payer],
            self.ctx.rpc().get_latest_blockhash().await.unwrap(),
        );

        self.ctx
            .rpc()
            .send_and_confirm_transaction(tx.into())
            .await?;

        Ok((
            MintInfo {
                address: Pubkey::find_program_address(
                    &[TOKEN_MINT, params.name.as_bytes()],
                    &glow_test_service::ID,
                )
                .0,
                is_token_2022,
            },
            params.price_oracle,
        ))
    }

    /// Register token
    pub async fn register_token(
        &self,
        mint: MintInfo,
        params: &TokenCreateParams,
        initial_price: i64,
    ) -> std::result::Result<(MintInfo, TokenPriceOracle), Error> {
        let payer = self.ctx.payer();

        let ix_token_create = test_service::token_register(&payer.pubkey(), mint, params);
        let ix_pyth_update_price = test_service::token_update_pyth_price(
            &payer.pubkey(),
            &mint.address,
            *params.price_oracle.pyth_feed_id().unwrap(),
            initial_price,
            100,
            -8,
        );

        let tx = Transaction::new_signed_with_payer(
            &[ix_token_create, ix_pyth_update_price],
            Some(&payer.pubkey()),
            &[payer],
            self.ctx.rpc().get_latest_blockhash().await.unwrap(),
        );

        self.ctx
            .rpc()
            .send_and_confirm_transaction(tx.into())
            .await?;

        Ok((mint, params.price_oracle))
    }

    /// Create a token with a Pyth pull oracle
    pub async fn create_native_token(
        &self,
        params: &TokenCreateParams,
        initial_price: i64,
    ) -> std::result::Result<(MintInfo, TokenPriceOracle), Error> {
        let payer = self.ctx.payer();

        let ix_token_create =
            test_service::token_init_native(&payer.pubkey(), &params.oracle_authority);
        let ix_pyth_update_price = test_service::token_update_pyth_price(
            &payer.pubkey(),
            &anchor_spl::token::spl_token::native_mint::ID,
            *params.price_oracle.pyth_feed_id().unwrap(),
            initial_price,
            100,
            -8,
        );

        let tx = Transaction::new_signed_with_payer(
            &[ix_token_create, ix_pyth_update_price],
            Some(&payer.pubkey()),
            &[payer],
            self.ctx.rpc().get_latest_blockhash().await.unwrap(),
        );

        self.ctx
            .rpc()
            .send_and_confirm_transaction(tx.into())
            .await?;

        Ok((
            MintInfo {
                address: anchor_spl::token::spl_token::native_mint::ID,
                is_token_2022: false,
            },
            params.price_oracle,
        ))
    }

    pub async fn create_token_from(
        &self,
        keypair: Keypair,
        decimals: u8,
        mint_authority: Option<&Pubkey>,
        freeze_authority: Option<&Pubkey>,
        is_token_2022: bool,
    ) -> std::result::Result<MintInfo, Error> {
        let payer = self.ctx.payer();
        let space = if is_token_2022 {
            anchor_spl::token_2022::spl_token_2022::state::Mint::LEN
        } else {
            anchor_spl::token::spl_token::state::Mint::LEN
        };
        let rent_lamports = self
            .ctx
            .rpc()
            .get_minimum_balance_for_rent_exemption(space)
            .await?;

        let tkn_program = &get_token_program(is_token_2022);
        let ix_create_account = system_instruction::create_account(
            &payer.pubkey(),
            &keypair.pubkey(),
            rent_lamports,
            space as u64,
            tkn_program,
        );

        let ix_initialize = spl_token_2022::instruction::initialize_mint(
            tkn_program,
            &keypair.pubkey(),
            mint_authority.unwrap_or(&payer.pubkey()),
            freeze_authority,
            decimals,
        )?;

        let tx = Transaction::new_signed_with_payer(
            &[ix_create_account, ix_initialize],
            Some(&payer.pubkey()),
            &[payer, &keypair],
            self.ctx.rpc().get_latest_blockhash().await.unwrap(),
        );

        self.ctx
            .rpc()
            .send_and_confirm_transaction(tx.into())
            .await?;

        Ok(MintInfo {
            address: keypair.pubkey(),
            is_token_2022,
        })
    }

    /// Create a new token account belonging to the owner, with the supplied mint
    pub async fn create_account(
        &self,
        mint: MintInfo,
        owner: &Pubkey,
    ) -> std::result::Result<Pubkey, Error> {
        // let ctx = self.ctx;
        let keypair = self.ctx.generate_key();
        let payer = self.ctx.payer();
        let space = if mint.is_token_2022 {
            anchor_spl::token_2022::spl_token_2022::state::Account::LEN
        } else {
            anchor_spl::token::spl_token::state::Account::LEN
        };
        let rent_lamports = self
            .ctx
            .rpc()
            .get_minimum_balance_for_rent_exemption(space)
            .await?;

        let ix_create_account = system_instruction::create_account(
            &payer.pubkey(),
            &keypair.pubkey(),
            rent_lamports,
            space as u64,
            &mint.token_program(),
        );

        let ix_initialize = if mint.is_token_2022 {
            Some(spl_token_2022::instruction::initialize_account(
                &mint.token_program(),
                &keypair.pubkey(),
                &mint.address,
                owner,
            )?)
        } else {
            Some(spl_token::instruction::initialize_account(
                &mint.token_program(),
                &keypair.pubkey(),
                &mint.address,
                owner,
            )?)
        };

        send_and_confirm(
            &self.ctx.rpc(),
            &[ix_create_account, ix_initialize.unwrap()],
            &[&keypair],
        )
        .await?;

        Ok(keypair.pubkey())
    }

    /// Create a new token account with some initial balance
    pub async fn create_account_funded(
        &self,
        mint: MintInfo,
        owner: &Pubkey,
        amount: u64,
    ) -> std::result::Result<Pubkey, Error> {
        let account = self.create_account(mint, owner).await?;
        if amount > 0 {
            self.mint(mint, owner, &account, amount).await?;
        }

        Ok(account)
    }

    /// Create a funded associated token account
    pub async fn create_associated_token_funded(
        &self,
        mint: MintInfo,
        owner: &Pubkey,
        amount: u64,
    ) -> std::result::Result<Pubkey, Error> {
        let account = self.create_associated_token(mint, owner).await?;
        if amount > 0 {
            self.mint(mint, owner, &account, amount).await?;
        }

        Ok(account)
    }

    /// Mint tokens to an account
    pub async fn mint(
        &self,
        mint: MintInfo,
        owner: &Pubkey,
        destination: &Pubkey,
        amount: u64,
    ) -> std::result::Result<(), Error> {
        let payer = self.ctx.payer();
        let ix_token_request =
            test_service::token_request(&payer.pubkey(), owner, mint, destination, amount);

        send_and_confirm(&self.ctx.rpc(), &[ix_token_request], &[]).await?;

        Ok(())
    }

    pub async fn refresh_to_same_price(
        &self,
        mint: &Pubkey,
        oracle: TokenPriceOracle,
    ) -> std::result::Result<(), Error> {
        self.ctx
            .rpc()
            .send_and_confirm(self.refresh_to_same_price_tx(mint, oracle).await?)
            .await?;

        Ok(())
    }

    pub async fn refresh_to_same_price_tx(
        &self,
        mint: &Pubkey,
        oracle: TokenPriceOracle,
    ) -> std::result::Result<TransactionBuilder, Error> {
        let price_address = derive_pyth_price_feed_account(
            oracle.pyth_feed_id().unwrap(),
            None,
            glow_test_service::ID,
        );
        let mut account: PriceUpdateV2 = self.get_pyth_price_update(&price_address).await?;

        let clock = self
            .ctx
            .rpc()
            .get_clock()
            .await
            .expect("could not get the clock");
        account.posted_slot = clock.slot;
        account.price_message.prev_publish_time = account.price_message.publish_time;
        account.price_message.publish_time = clock.unix_timestamp;

        Ok(self.set_price_tx(
            mint,
            &TokenPrice {
                feed_id: *oracle.pyth_feed_id().unwrap(),
                price: account.price_message.price,
                exponent: account.price_message.exponent,
                confidence: account.price_message.conf,
                twap: account.price_message.ema_price as u64,
            },
        ))
    }

    /// Set the oracle price of a token
    pub async fn set_price(
        &self,
        mint: &Pubkey,
        price: &TokenPrice,
    ) -> std::result::Result<(), Error> {
        self.ctx
            .rpc()
            .send_and_confirm(self.set_price_tx(mint, price))
            .await?;

        Ok(())
    }

    /// Set the oracle price of a token
    pub fn set_price_tx(&self, mint: &Pubkey, price: &TokenPrice) -> TransactionBuilder {
        TransactionBuilder {
            instructions: vec![test_service::token_update_pyth_price(
                &self.ctx.payer().pubkey(),
                mint,
                price.feed_id,
                price.price,
                price.confidence as i64,
                price.exponent,
            )],
            signers: vec![],
        }
    }

    /// Get the current balance of a token account
    pub async fn get_balance(&self, account: &Pubkey) -> std::result::Result<u64, Error> {
        let account_data = self.ctx.rpc().get_account(account).await?;
        // See: https://solana.stackexchange.com/questions/8308/check-if-an-account-is-a-token-account
        let data = account_data.context("No data when getting balance")?;
        let account_len = anchor_spl::token::spl_token::state::Account::LEN;
        let state = if data.data.len() > account_len {
            // Check that this is a valid token account still
            // if data.data.get(account_len + 1) == Some(&0x02u8) {
            anchor_spl::token_2022::spl_token_2022::state::Account::unpack(
                &data.data()[0..account_len],
            )?
            // } else {
            //     bail!("Invalid token account, the account might be token-2022 but its data is invalid")
            // }
        } else {
            anchor_spl::token_2022::spl_token_2022::state::Account::unpack(&data.data)?
        };

        Ok(state.amount)
    }

    /// Get the mint by its pubkey
    pub async fn get_mint(&self, account: &Pubkey) -> std::result::Result<Mint, Error> {
        let account_data = self.ctx.rpc().get_account(account).await?;

        let state = Mint::unpack(&account_data.unwrap().data)?;

        Ok(state)
    }

    /// Wrap SOL in a wallet's ATA
    pub async fn wrap_native(&self, wallet: &Keypair, amount: u64) -> anyhow::Result<Pubkey> {
        let native_mint = MintInfo::native();
        let wallet_address = wallet.pubkey();
        let address = native_mint.associated_token_address(&wallet_address);
        let ata_ix = native_mint
            .create_associated_token_account_idempotent(&wallet_address, &wallet_address);
        let transfer_ix = system_instruction::transfer(&wallet_address, &address, amount);
        let sync_ix = anchor_spl::token::spl_token::instruction::sync_native(
            &native_mint.token_program(),
            &address,
        )?;

        send_and_confirm(&self.ctx.rpc(), &[ata_ix, transfer_ix, sync_ix], &[wallet]).await?;

        Ok(address)
    }

    pub async fn get_pyth_price_update(
        &self,
        address: &Pubkey,
    ) -> std::result::Result<PriceUpdateV2, Error> {
        let rpc = self.ctx.rpc();
        let account =
            with_retries_and_timeout(|| rpc.get_account(address), Duration::from_secs(1), 30)
                .await?
                .unwrap()
                .unwrap();
        Ok(PriceUpdateV2::try_deserialize(&mut &account.data[..])?)
    }
}
