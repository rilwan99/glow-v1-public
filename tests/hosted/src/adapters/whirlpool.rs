//! Set up whirlpool and set liquidity

use anchor_lang::{prelude::*, system_program, InstructionData};
use anchor_spl::associated_token::{
    get_associated_token_address, get_associated_token_address_with_program_id,
};
use glow_instructions::MintInfo;
use num_traits::{Pow, ToPrimitive};
use rust_decimal::{Decimal, MathematicalOps};
use solana_sdk::{
    instruction::Instruction, pubkey::Pubkey, signature::Keypair, signer::Signer, sysvar::SysvarId,
};
use solana_test_framework::ClientExtensions;
use whirlpool::math::{
    mul_u256, sqrt_price_from_tick_index, tick_index_from_sqrt_price, U256Muldiv,
};
use whirlpool::state::{
    OpenPositionBumps, WhirlpoolBumps, MAX_TICK_INDEX, MIN_TICK_INDEX, TICK_ARRAY_SIZE,
};

use glow_simulation::Keygen;
use glow_simulation::{runtime::TestRuntimeRpcClient, DeterministicKeygen};

use crate::tokens::TokenManager;

pub struct TestWhirlpool {
    pub address: Pubkey,
    pub authority: Pubkey,
    pub lp_mint: Pubkey,
    pub config: Keypair,
    pub mint_a: MintInfo,
    pub mint_b: MintInfo,
    pub vault_a: Keypair,
    pub vault_b: Keypair,
    pub position_mint: Keypair,
    pub tick_arrays: [Pubkey; 5],
}

impl TestWhirlpool {
    pub async fn create(
        context: &TestRuntimeRpcClient,
        mint_a: MintInfo,
        mint_b: MintInfo,
        decimal_a: u8,
        decimal_b: u8,
        lp_mint: Pubkey,
    ) -> anyhow::Result<Self> {
        // Mint some tokens for the user to add liquidity with
        let authority = context.payer().pubkey();
        let token_owner_account_a = mint_a.associated_token_address(&authority);
        let token_owner_account_b = mint_b.associated_token_address(&authority);

        {
            let mut c = context.context.write().await;
            c.banks_client
                .create_associated_token_account(
                    &authority,
                    &mint_a.address,
                    context.payer(),
                    &mint_a.token_program(),
                )
                .await
                .unwrap();
            c.banks_client
                .create_associated_token_account(
                    &authority,
                    &mint_b.address,
                    context.payer(),
                    &mint_b.token_program(),
                )
                .await
                .unwrap();
            drop(c);
            let token_manager = TokenManager::new(context.clone());
            token_manager
                .mint(mint_a, &authority, &token_owner_account_a, 100_000_000_000)
                .await?;
            token_manager
                .mint(mint_b, &authority, &token_owner_account_b, 100_000_000_000)
                .await?;
        }
        let tick_spacing = 1u16;
        let default_fee_rate = 300;
        let initial_price = 148.153774;

        // Initialize config
        let config = context.keygen.generate_key();
        let mut ixs = vec![];
        let init_ix = Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::InitializeConfig {
                config: config.pubkey(),
                funder: authority,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: whirlpool::instruction::InitializeConfig {
                fee_authority: authority,
                collect_protocol_fees_authority: authority,
                reward_emissions_super_authority: authority,
                default_protocol_fee_rate: default_fee_rate,
            }
            .data(),
        };
        ixs.push(init_ix);
        // .. fee tier
        let fee_tier = Pubkey::find_program_address(
            &[
                b"fee_tier",
                config.pubkey().as_ref(),
                tick_spacing.to_le_bytes().as_ref(),
            ],
            &whirlpool::ID,
        );
        let fee_tier_ix = Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::InitializeFeeTier {
                config: config.pubkey(),
                fee_tier: fee_tier.0,
                funder: authority,
                fee_authority: authority,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: whirlpool::instruction::InitializeFeeTier {
                tick_spacing,
                default_fee_rate,
            }
            .data(),
        };
        ixs.push(fee_tier_ix);
        // .. pool
        let vault_a = context.keygen.generate_key();
        let vault_b = context.keygen.generate_key();
        let whirlpool_address = Pubkey::find_program_address(
            &[
                b"whirlpool",
                config.pubkey().as_ref(),
                mint_a.address.as_ref(),
                mint_b.address.as_ref(),
                tick_spacing.to_le_bytes().as_ref(),
            ],
            &whirlpool::ID,
        );
        let pool_ix = Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::InitializePool {
                whirlpools_config: config.pubkey(),
                token_mint_a: mint_a.address,
                token_mint_b: mint_b.address,
                funder: authority,
                whirlpool: whirlpool_address.0,
                token_vault_a: vault_a.pubkey(),
                token_vault_b: vault_b.pubkey(),
                fee_tier: fee_tier.0,
                token_program: anchor_spl::token::ID,
                system_program: system_program::ID,
                rent: Rent::id(),
            }
            .to_account_metas(None),
            data: whirlpool::instruction::InitializePool {
                tick_spacing,
                // initial_sqrt_price: 1 << 64,
                initial_sqrt_price: dbg!(price_to_sqrt_price(initial_price, decimal_a, decimal_b)),
                bumps: WhirlpoolBumps {
                    whirlpool_bump: whirlpool_address.1,
                },
            }
            .data(),
        };
        ixs.push(pool_ix);
        // .. ticks
        let midpoint = price_to_tick_index(145.569208, decimal_a, decimal_b, tick_spacing);
        let midpoint = start_tick_index(midpoint, tick_spacing, 0);
        let [tick_m2, tick_lower_index, tick_middle_index, tick_upper_index, tick_p2] = [
            midpoint - tick_spacing as i32 * TICK_ARRAY_SIZE * 2,
            midpoint - tick_spacing as i32 * TICK_ARRAY_SIZE,
            midpoint,
            midpoint + tick_spacing as i32 * TICK_ARRAY_SIZE,
            midpoint + tick_spacing as i32 * TICK_ARRAY_SIZE * 2,
        ];
        let tick_array_m2 = derive_tick_array(&whirlpool_address.0, tick_m2, tick_spacing);
        let tick_array_lower =
            derive_tick_array(&whirlpool_address.0, tick_lower_index, tick_spacing);
        let tick_array_middle =
            derive_tick_array(&whirlpool_address.0, tick_middle_index, tick_spacing);
        let tick_array_upper =
            derive_tick_array(&whirlpool_address.0, tick_upper_index, tick_spacing);
        let tick_array_p2 = derive_tick_array(&whirlpool_address.0, tick_p2, tick_spacing);
        ixs.push(Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::InitializeTickArray {
                whirlpool: whirlpool_address.0,
                funder: authority,
                tick_array: tick_array_m2,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: whirlpool::instruction::InitializeTickArray {
                start_tick_index: start_tick_index(tick_m2, tick_spacing, 0),
            }
            .data(),
        });
        ixs.push(Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::InitializeTickArray {
                whirlpool: whirlpool_address.0,
                funder: authority,
                tick_array: tick_array_lower,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: whirlpool::instruction::InitializeTickArray {
                start_tick_index: start_tick_index(tick_lower_index, tick_spacing, 0),
            }
            .data(),
        });
        ixs.push(Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::InitializeTickArray {
                whirlpool: whirlpool_address.0,
                funder: authority,
                tick_array: tick_array_middle,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: whirlpool::instruction::InitializeTickArray {
                start_tick_index: start_tick_index(tick_middle_index, tick_spacing, 0),
            }
            .data(),
        });
        ixs.push(Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::InitializeTickArray {
                whirlpool: whirlpool_address.0,
                funder: authority,
                tick_array: tick_array_upper,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: whirlpool::instruction::InitializeTickArray {
                start_tick_index: start_tick_index(tick_upper_index, tick_spacing, 0),
            }
            .data(),
        });
        ixs.push(Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::InitializeTickArray {
                whirlpool: whirlpool_address.0,
                funder: authority,
                tick_array: tick_array_p2,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: whirlpool::instruction::InitializeTickArray {
                start_tick_index: start_tick_index(tick_p2, tick_spacing, 0),
            }
            .data(),
        });
        // create a position
        let position_mint = context.keygen.generate_key();
        let (position, position_bump) = Pubkey::find_program_address(
            &[b"position", position_mint.pubkey().as_ref()],
            &whirlpool::ID,
        );
        let position_token_account = get_associated_token_address_with_program_id(
            &authority,
            &position_mint.pubkey(),
            &anchor_spl::token::ID,
        );
        let position_ix = Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::OpenPosition {
                funder: authority,
                owner: authority,
                position,
                position_mint: position_mint.pubkey(),
                position_token_account,
                whirlpool: whirlpool_address.0,
                token_program: anchor_spl::token::ID,
                system_program: system_program::ID,
                rent: Rent::id(),
                associated_token_program: anchor_spl::associated_token::ID,
            }
            .to_account_metas(None),
            data: whirlpool::instruction::OpenPosition {
                bumps: OpenPositionBumps { position_bump },
                tick_lower_index,
                tick_upper_index,
            }
            .data(),
        };
        ixs.push(position_ix);
        // add liquidity
        let add_liquidity_ix = Instruction {
            program_id: whirlpool::ID,
            accounts: whirlpool::accounts::ModifyLiquidity {
                whirlpool: whirlpool_address.0,
                token_program: anchor_spl::token::ID,
                position_authority: authority,
                position,
                position_token_account,
                token_owner_account_a,
                token_owner_account_b,
                token_vault_a: vault_a.pubkey(),
                token_vault_b: vault_b.pubkey(),
                tick_array_lower,
                tick_array_upper,
            }
            .to_account_metas(None),
            data: whirlpool::instruction::IncreaseLiquidity {
                liquidity_amount: 100_000_000, // an arbitrary number, we can calculate it if we'd like
                token_max_a: u64::MAX,
                token_max_b: u64::MAX,
            }
            .data(),
        };
        ixs.push(add_liquidity_ix);

        let tx = context
            .create_transaction(
                &ixs,
                context.payer(),
                vec![context.payer(), &config, &vault_a, &vault_b, &position_mint],
            )
            .await?;
        context.send_and_confirm(tx).await?;

        Ok(Self {
            address: whirlpool_address.0,
            authority,
            config,
            mint_a,
            mint_b,
            vault_a,
            vault_b,
            position_mint,
            lp_mint,
            tick_arrays: [
                tick_array_m2,
                tick_array_lower,
                tick_array_middle,
                tick_array_upper,
                tick_array_p2,
            ],
        })
    }
}

pub fn start_tick_index(tick_index: i32, tick_spacing: u16, offset: i32) -> i32 {
    let index_real = tick_index as f64 / tick_spacing as f64 / TICK_ARRAY_SIZE as f64;
    (index_real.floor() as i32 + offset) * tick_spacing as i32 * TICK_ARRAY_SIZE
}

fn price_to_tick_index(
    price: f64,
    decimals_a: impl Into<i64>,
    decimals_b: impl Into<i64>,
    tick_spacing: u16,
) -> i32 {
    let sqrt_price = price_to_sqrt_price(price, decimals_a, decimals_b);
    sqrt_price_to_tick_index(&sqrt_price, tick_spacing)
}

fn price_to_sqrt_price(price: f64, decimals_a: impl Into<i64>, decimals_b: impl Into<i64>) -> u128 {
    let c = Decimal::TEN.pow(decimals_b.into() - decimals_a.into());
    let price = Decimal::from_f64_retain(price).unwrap();
    let sqrt_price = (price * c).sqrt().unwrap() * Decimal::from(2).powi(64);

    sqrt_price.floor().to_u128().unwrap()
}

fn sqrt_price_to_tick_index(sqrt_price: &u128, tick_spacing: u16) -> i32 {
    let tick_index = whirlpool::math::tick_math::tick_index_from_sqrt_price(sqrt_price);
    tick_index - (tick_index % tick_spacing as i32)
}

#[allow(unused)]
fn liquidity_for_token_a(amount: u64, min_sqrt_price: u128, max_sqrt_price: u128) -> u128 {
    let div = max_sqrt_price - min_sqrt_price;
    let liquidity = mul_u256(max_sqrt_price, min_sqrt_price)
        .mul(U256Muldiv::new(0, amount as u128))
        .div(U256Muldiv::new(0, div).shift_left(64), false)
        .0;

    liquidity.get_word_u128(0)
}

#[allow(unused)]
fn liquidity_for_token_b(amount: u64, min_sqrt_price: u128, max_sqrt_price: u128) -> u128 {
    ((amount as u128) << 64) / (max_sqrt_price - min_sqrt_price)
}

pub fn derive_tick_array(whirlpool: &Pubkey, tick_index: i32, tick_spacing: u16) -> Pubkey {
    assert!(tick_index >= MIN_TICK_INDEX);
    assert!(tick_index <= MAX_TICK_INDEX);

    let start_tick_index = start_tick_index(tick_index, tick_spacing, 0);

    Pubkey::find_program_address(
        &[
            b"tick_array",
            whirlpool.as_ref(),
            start_tick_index.to_string().as_bytes(),
        ],
        &whirlpool::ID,
    )
    .0
}
