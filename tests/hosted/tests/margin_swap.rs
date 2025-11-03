use std::str::FromStr;

use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anyhow::Error;

use glow_instructions::MintInfo;
use glow_margin::{AccountFeatureFlags, TokenKind};
use glow_margin_pool::{MarginPoolConfig, PoolFlags};
use glow_margin_sdk::{
    ix_builder::MarginPoolIxBuilder,
    solana::transaction::{TransactionBuilderExt, WithSigner},
    tokens::TokenPrice,
    tx_builder::{MarginActionAuthority, TokenDepositsConfig},
};
use glow_program_common::{oracle::TokenPriceOracle, token_change::TokenChange};
use hosted_tests::{
    adapters::saber::TestSaberPool, program_test::JUP_V6, send_and_confirm,
    tokens::preset_token_configs::*,
};

use jupiter_cpi::jupiter_override::{self, RoutePlanStep};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use solana_sdk::{instruction::Instruction, native_token::LAMPORTS_PER_SOL};

use anchor_lang::{InstructionData, ToAccountMetas};
use hosted_tests::{context::MarginTestContext, margin::MarginPoolSetupInfo, margin_test_context};

const ONE_USDC: u64 = 1_000_000;
const ONE_USDT: u64 = 1_000_000;
const ONE_TSOL: u64 = LAMPORTS_PER_SOL;

const DEFAULT_POOL_CONFIG: MarginPoolConfig = MarginPoolConfig {
    borrow_rate_0: 10,
    borrow_rate_1: 20,
    borrow_rate_2: 30,
    borrow_rate_3: 40,
    utilization_rate_1: 10,
    utilization_rate_2: 20,
    management_fee_rate: 10,
    flags: PoolFlags::ALLOW_LENDING.bits(),
    deposit_limit: u64::MAX,
    borrow_limit: u64::MAX,
    reserved: 0,
};

struct TestEnv {
    usdc: MintInfo,
    tsol: MintInfo,
    usdt: MintInfo,
    usdc_oracle: TokenPriceOracle,
    usdt_oracle: TokenPriceOracle,
    tsol_oracle: TokenPriceOracle,
}

async fn setup_environment(ctx: &MarginTestContext) -> Result<TestEnv, Error> {
    let (usdc, usdc_oracle) = ctx
        .tokens()
        .create_token_v2(
            &usdc_config(ctx.mint_authority().pubkey()),
            100_000_000,
            false,
        )
        .await?;
    let (usdt, usdt_oracle) = ctx
        .tokens()
        .create_token_v2(
            &usdt_config(ctx.mint_authority().pubkey()),
            100_000_000,
            false,
        )
        .await?;
    let (tsol, tsol_oracle) = ctx
        .tokens()
        .create_token_v2(
            &tsol_config(ctx.mint_authority().pubkey()),
            20_000_000_000,
            false,
        )
        .await?;

    let pools = [
        MarginPoolSetupInfo {
            mint_info: usdc,
            token_kind: TokenKind::Collateral,
            collateral_weight: 1_00,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: usdc_oracle,
            max_staleness: 30,
            token_features: Default::default(),
        },
        MarginPoolSetupInfo {
            mint_info: usdt,
            token_kind: TokenKind::Collateral,
            collateral_weight: 1_00,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: usdt_oracle,
            max_staleness: 30,
            token_features: Default::default(),
        },
        MarginPoolSetupInfo {
            mint_info: tsol,
            token_kind: TokenKind::Collateral,
            collateral_weight: 95,
            max_leverage: 4_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: tsol_oracle,
            max_staleness: 30,
            token_features: Default::default(),
        },
    ];

    for pool_info in pools {
        ctx.margin_client()
            .configure_token_deposits(
                pool_info.mint_info,
                Some(&TokenDepositsConfig {
                    oracle: pool_info.oracle,
                    collateral_weight: pool_info.collateral_weight,
                    max_staleness: 30,
                    token_features: Default::default(),
                }),
            )
            .await?;
        ctx.margin_client().create_pool(&pool_info).await?;
    }

    Ok(TestEnv {
        usdc,
        tsol,
        usdt,
        usdc_oracle,
        usdt_oracle,
        tsol_oracle,
    })
}

/// Margin swap using Jupiter via Saber
#[tokio::test(flavor = "multi_thread")]
async fn margin_swap_jupiter_saber() -> Result<(), anyhow::Error> {
    // Get the mocked runtime
    let ctx = margin_test_context!();

    // create position metadata refresher
    let refresher = ctx.generate_key();
    ctx.margin_config_ix()
        .configure_position_config_refresher(refresher.pubkey(), true)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let env = setup_environment(&ctx).await?;

    // Register Jupiter as an adapter
    ctx.margin_client().register_adapter(&JUP_V6).await?;

    // Set up Whirlpool
    let saber = TestSaberPool::create(&ctx.solana, env.usdt, env.usdc).await?;

    // Create our two user wallets, with some SOL funding to get started
    let wallet_a = ctx.create_wallet(10).await?;
    let wallet_b = ctx.create_wallet(10).await?;

    // issue permits for the users
    ctx.issue_permit(wallet_a.pubkey()).await?;
    ctx.issue_permit(wallet_b.pubkey()).await?;

    // Create the user context helpers, which give a simple interface for executing
    // common actions on a margin account
    let user_a = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::default())
        .await?;
    let user_b = ctx
        .margin_client()
        .user(&wallet_b, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::default())
        .await?;

    let usdc_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.usdc);
    let usdt_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.usdt);
    let tsol_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.tsol);

    for _ in 0..10 {
        ctx.solana
            .context_mut()
            .await
            .warp_forward_force_reward_interval_end()?;
    }

    user_a.init_lookup_registry().await?;
    let lookup_table = user_a.create_lookup_table().await?;
    // Extend the lookup table with useful addresses
    user_a
        .append_to_lookup_table(
            lookup_table,
            &[
                // Programs
                glow_margin::ID,
                glow_margin_pool::ID,
                glow_metadata::ID,
                anchor_spl::token::ID,
                anchor_spl::associated_token::ID,
                jupiter_cpi::ID,
                // wallet and margin account
                *user_a.address(),
                *user_a.owner(),
                // Mints, pools
                env.usdc.address,
                env.tsol.address,
                env.usdt.address,
                usdc_pool.address,
                usdc_pool.deposit_note_mint,
                usdc_pool.loan_note_mint,
                usdc_pool.vault,
                usdt_pool.address,
                usdt_pool.deposit_note_mint,
                usdt_pool.loan_note_mint,
                usdt_pool.vault,
                tsol_pool.address,
                tsol_pool.deposit_note_mint,
                tsol_pool.loan_note_mint,
                tsol_pool.vault,
                // ATAs
                get_associated_token_address_with_program_id(
                    user_a.address(),
                    &env.usdc.address,
                    &env.usdc.token_program(),
                ),
                get_associated_token_address_with_program_id(
                    user_a.address(),
                    &env.usdt.address,
                    &env.usdt.token_program(),
                ),
                get_associated_token_address_with_program_id(
                    user_a.address(),
                    &env.tsol.address,
                    &env.tsol.token_program(),
                ),
            ],
        )
        .await?;

    user_a.refresh_lookup_tables().await?;

    // Wait for lookup table to become active
    for _ in 0..40 {
        ctx.solana
            .context_mut()
            .await
            .warp_forward_force_reward_interval_end()?;
    }

    // Create some tokens for each user to deposit
    let user_a_tsol_account = ctx
        .tokens()
        .create_account_funded(env.tsol, &wallet_a.pubkey(), 1_000 * ONE_TSOL)
        .await?;
    let user_b_usdc_account = ctx
        .tokens()
        .create_account_funded(env.usdc, &wallet_b.pubkey(), 1_000_000 * ONE_USDC)
        .await?;

    // Set the prices for each token
    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &env.usdc.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000,
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *env.usdc_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &env.usdt.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000,
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *env.usdt_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 100 USD +- 1
            &env.tsol.address,
            &TokenPrice {
                exponent: -8,
                price: 10_000_000_000,
                confidence: 100_000_000,
                twap: 10_000_000_000,
                feed_id: *env.tsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    // Deposit user funds into their margin accounts
    let usdc_deposit_amount = 1_000_000 * ONE_USDC;
    let tsol_deposit_amount = 1_000 * ONE_TSOL;

    // User A only deposits TSOL
    // User B deposits USDC
    // User A will borrow the USDT from the pool and swap for USDC
    user_a
        .pool_deposit(
            env.tsol,
            Some(user_a_tsol_account),
            TokenChange::shift(tsol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    user_b
        .pool_deposit(
            env.usdc,
            Some(user_b_usdc_account),
            TokenChange::shift(usdc_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // Verify user tokens have been deposited
    assert_eq!(0, ctx.tokens().get_balance(&user_a_tsol_account).await?);
    assert_eq!(0, ctx.tokens().get_balance(&user_b_usdc_account).await?);

    user_a.refresh_all_pool_positions().await?;
    user_b.refresh_all_pool_positions().await?;
    user_a.refresh_all_position_metadata(&refresher).await?;

    let event_authority = Pubkey::from_str("D8cy77BBepLMngZx6ZukaTff5hCt1HrWyKk3Hnd9oitf").unwrap();

    let borrow_dst = env.usdc.associated_token_address(user_a.address());
    let deposit_src = env.usdt.associated_token_address(user_a.address());

    let mut accounts = jupiter_cpi::accounts::Route {
        token_program: anchor_spl::token::ID,
        user_transfer_authority: *user_a.address(),
        user_source_token_account: borrow_dst,
        user_destination_token_account: deposit_src,
        event_authority,
        program: jupiter_cpi::id(),
        destination_token_account: jupiter_cpi::id(),
        destination_mint: env.usdt.address,
        platform_fee_account: jupiter_cpi::id(),
    }
    .to_account_metas(None);

    let saber_accounts = jupiter_cpi::accounts::SaberSwap {
        swap_program: stable_swap_client::ID,
        token_program: anchor_spl::token::ID,
        swap: saber.address,
        swap_authority: saber.authority,
        user_authority: *user_a.address(),
        input_user_account: borrow_dst,
        input_token_account: saber.reserve_b,
        output_token_account: saber.reserve_a,
        output_user_account: deposit_src,
        fees_token_account: saber.admin_fees_a,
    };
    accounts.extend_from_slice(&saber_accounts.to_account_metas(None));

    // This was taken as am example: https://solscan.io/tx/56CWBWyrSti1jCs1jju8C9uZBfXVmatKCxjX7XmiiiSY374CACu1o14zmgkLcrwn58KLndqCurwGYYzojYrcdpYq
    // Jupiter IDL: https://solscan.io/account/JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4#anchorProgramIdl
    // Create CPI swap instruction
    let swap_ix = Instruction {
        program_id: jupiter_cpi::id(),
        accounts,
        data: jupiter_override::Route {
            route_plan: vec![RoutePlanStep {
                swap: jupiter_amm_interface::Swap::Saber,
                percent: 100,
                input_index: 0,
                output_index: 1,
            }],
            in_amount: 99 * ONE_USDT,
            quoted_out_amount: Default::default(),
            slippage_bps: 50,
            platform_fee_bps: Default::default(),
        }
        .data(),
    };

    user_a
        .margin_swap(env.usdc, env.usdt, 100 * ONE_USDC, &swap_ix)
        .await?;

    Ok(())
}

/// Margin swap that goes directly via Saber
#[tokio::test(flavor = "multi_thread")]
async fn margin_swap_saber() -> Result<(), anyhow::Error> {
    // Get the mocked runtime
    let ctx = margin_test_context!();

    // create position metadata refresher
    let refresher = ctx.generate_key();
    ctx.margin_config_ix()
        .configure_position_config_refresher(refresher.pubkey(), true)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let env = setup_environment(&ctx).await?;

    // Set up Whirlpool
    let saber = TestSaberPool::create(&ctx.solana, env.usdt, env.usdc).await?;

    // Create our two user wallets, with some SOL funding to get started
    let wallet_a = ctx.create_wallet(10).await?;
    let wallet_b = ctx.create_wallet(10).await?;

    // issue permits for the users
    ctx.issue_permit(wallet_a.pubkey()).await?;
    ctx.issue_permit(wallet_b.pubkey()).await?;

    // Create the user context helpers, which give a simple interface for executing
    // common actions on a margin account
    let user_a = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::default())
        .await?;
    let user_b = ctx
        .margin_client()
        .user(&wallet_b, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::default())
        .await?;

    let usdc_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.usdc);
    let usdt_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.usdt);
    let tsol_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.tsol);

    for _ in 0..10 {
        ctx.solana
            .context_mut()
            .await
            .warp_forward_force_reward_interval_end()?;
    }

    user_a.init_lookup_registry().await?;
    let lookup_table = user_a.create_lookup_table().await?;
    // Extend the lookup table with useful addresses
    user_a
        .append_to_lookup_table(
            lookup_table,
            &[
                // Programs
                glow_margin::ID,
                glow_margin_pool::ID,
                glow_metadata::ID,
                anchor_spl::token::ID,
                anchor_spl::associated_token::ID,
                stable_swap_client::ID,
                // wallet and margin account
                *user_a.address(),
                *user_a.owner(),
                // Mints, pools
                env.usdc.address,
                env.tsol.address,
                env.usdt.address,
                usdc_pool.address,
                usdc_pool.deposit_note_mint,
                usdc_pool.loan_note_mint,
                usdc_pool.vault,
                usdt_pool.address,
                usdt_pool.deposit_note_mint,
                usdt_pool.loan_note_mint,
                usdt_pool.vault,
                tsol_pool.address,
                tsol_pool.deposit_note_mint,
                tsol_pool.loan_note_mint,
                tsol_pool.vault,
                // ATAs
                env.usdc.associated_token_address(user_a.address()),
                env.usdt.associated_token_address(user_a.address()),
                env.tsol.associated_token_address(user_a.address()),
            ],
        )
        .await?;

    user_a.refresh_lookup_tables().await?;

    // Wait for lookup table to become active
    for _ in 0..10 {
        ctx.solana
            .context_mut()
            .await
            .warp_forward_force_reward_interval_end()?;
    }

    // Create some tokens for each user to deposit
    let user_a_tsol_account = ctx
        .tokens()
        .create_account_funded(env.tsol, &wallet_a.pubkey(), 1_000 * ONE_TSOL)
        .await?;
    let user_b_usdc_account = ctx
        .tokens()
        .create_account_funded(env.usdc, &wallet_b.pubkey(), 1_000_000 * ONE_USDC)
        .await?;

    // Set the prices for each token
    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &env.usdc.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000,
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *env.usdc_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &env.usdt.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000,
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *env.usdt_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 100 USD +- 1
            &env.tsol.address,
            &TokenPrice {
                exponent: -8,
                price: 10_000_000_000,
                confidence: 100_000_000,
                twap: 10_000_000_000,
                feed_id: *env.tsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    // Deposit user funds into their margin accounts
    let usdc_deposit_amount = 1_000_000 * ONE_USDC;
    let tsol_deposit_amount = 1_000 * ONE_TSOL;

    // User A only deposits TSOL
    // User B deposits USDC
    // User A will borrow the USDT from the pool and swap for USDC
    user_a
        .pool_deposit(
            env.tsol,
            Some(user_a_tsol_account),
            TokenChange::shift(tsol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    user_b
        .pool_deposit(
            env.usdc,
            Some(user_b_usdc_account),
            TokenChange::shift(usdc_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // Verify user tokens have been deposited
    assert_eq!(0, ctx.tokens().get_balance(&user_a_tsol_account).await?);
    assert_eq!(0, ctx.tokens().get_balance(&user_b_usdc_account).await?);

    user_a.refresh_all_pool_positions().await?;
    user_b.refresh_all_pool_positions().await?;
    user_a.refresh_all_position_metadata(&refresher).await?;

    let borrow_dst = env.usdc.associated_token_address(user_a.address());
    let deposit_src = env.usdt.associated_token_address(user_a.address());

    let saber_swap_ix = stable_swap_client::instruction::swap(
        &anchor_spl::token::ID,
        &saber.address,
        &saber.authority,
        user_a.address(),
        &borrow_dst,
        &saber.reserve_b,
        &saber.reserve_a,
        &deposit_src,
        &saber.admin_fees_a,
        100 * ONE_USDT,
        99 * ONE_USDC,
    )?;

    // let result = user_a
    //     .margin_swap(env.usdc, env.usdt, 100_000_000, 99_000_000, &saber_swap_ix.clone())
    //     .await;
    // // The first swap should fail because the Saber adapter has not been authorized as an adapter.
    // assert_custom_program_error(anchor_lang::error::ErrorCode::AccountNotInitialized, result);

    // Register Saber as an adapter
    ctx.margin_client()
        .register_adapter(&stable_swap_client::ID)
        .await?;

    user_a.refresh_all_pool_positions().await?;
    user_a
        .margin_swap(env.usdc, env.usdt, 100 * ONE_USDC, &saber_swap_ix.clone())
        .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_swap_jupiter_saber() -> Result<(), anyhow::Error> {
    // Get the mocked runtime
    let ctx = margin_test_context!();

    // create position metadata refresher
    let refresher = ctx.generate_key();
    ctx.margin_config_ix()
        .configure_position_config_refresher(refresher.pubkey(), true)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let env = setup_environment(&ctx).await?;

    // Register Jupiter as an adapter
    ctx.margin_client().register_adapter(&JUP_V6).await?;

    // Set up Whirlpool
    let saber = TestSaberPool::create(&ctx.solana, env.usdt, env.usdc).await?;

    // Create our two user wallets, with some SOL funding to get started
    let wallet = ctx.create_wallet(10).await?;

    let user_usdc_account = ctx
        .tokens()
        .create_account_funded(env.usdc, &wallet.pubkey(), 1_000_000 * ONE_USDC)
        .await?;
    let user_usdt_account = ctx
        .tokens()
        .create_account(env.usdt, &wallet.pubkey())
        .await?;

    let event_authority = Pubkey::from_str("D8cy77BBepLMngZx6ZukaTff5hCt1HrWyKk3Hnd9oitf").unwrap();

    let mut accounts = jupiter_cpi::accounts::Route {
        token_program: anchor_spl::token::ID,
        user_transfer_authority: wallet.pubkey(),
        user_source_token_account: user_usdc_account,
        user_destination_token_account: user_usdt_account,
        event_authority,
        program: jupiter_cpi::id(),
        destination_token_account: jupiter_cpi::id(),
        destination_mint: env.usdt.address,
        platform_fee_account: jupiter_cpi::id(),
    }
    .to_account_metas(None);

    let saber_accounts = jupiter_cpi::accounts::SaberSwap {
        swap_program: stable_swap_client::ID,
        token_program: anchor_spl::token::ID,
        swap: saber.address,
        swap_authority: saber.authority,
        user_authority: wallet.pubkey(),
        input_user_account: user_usdc_account,
        input_token_account: saber.reserve_b,
        output_token_account: saber.reserve_a,
        output_user_account: user_usdt_account,
        fees_token_account: saber.admin_fees_a,
    };
    accounts.extend_from_slice(&saber_accounts.to_account_metas(None));

    // This was taken as am example: https://solscan.io/tx/56CWBWyrSti1jCs1jju8C9uZBfXVmatKCxjX7XmiiiSY374CACu1o14zmgkLcrwn58KLndqCurwGYYzojYrcdpYq
    // Jupiter IDL: https://solscan.io/account/JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4#anchorProgramIdl
    // Create CPI swap instruction
    let swap_ix = Instruction {
        program_id: jupiter_cpi::id(),
        accounts,
        data: jupiter_override::Route {
            route_plan: vec![RoutePlanStep {
                swap: jupiter_amm_interface::Swap::Saber,
                percent: 100,
                input_index: 0,
                output_index: 1,
            }],
            in_amount: 100 * ONE_USDT,
            quoted_out_amount: Default::default(),
            slippage_bps: 50, //TODO replace with actual slippage
            platform_fee_bps: Default::default(),
        }
        .data(),
    };

    send_and_confirm(&ctx.rpc(), &[swap_ix], &[&wallet]).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn liquidator_swap() -> Result<(), anyhow::Error> {
    // A user borrows tokens, token prices become adverse, a liquidator liquidates them.
    // The liquidator should take a prescribed fee as part of the liquidation.
    // This fee should not be manipulated to be higher.

    // This test replicates much of margin_swap_jupiter_saber.

    // Get the mocked runtime
    let ctx = margin_test_context!();

    // create position metadata refresher
    let refresher = ctx.generate_key();
    ctx.margin_config_ix()
        .configure_position_config_refresher(refresher.pubkey(), true)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let env = setup_environment(&ctx).await?;

    // Register Jupiter as an adapter
    ctx.margin_client().register_adapter(&JUP_V6).await?;

    // Set up Saber
    let saber = TestSaberPool::create(&ctx.solana, env.usdt, env.usdc).await?;

    // Create our two user wallets, with some SOL funding to get started
    let wallet_a = ctx.create_wallet(10).await?;
    let wallet_b = ctx.create_wallet(10).await?;

    // issue permits for the users
    ctx.issue_permit(wallet_a.pubkey()).await?;
    ctx.issue_permit(wallet_b.pubkey()).await?;

    // Create the user context helpers, which give a simple interface for executing
    // common actions on a margin account
    let user_a = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::default())
        .await?;
    let user_b = ctx
        .margin_client()
        .user(&wallet_b, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::default())
        .await?;

    let usdc_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.usdc);
    let usdt_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.usdt);
    let tsol_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.tsol);

    for _ in 0..10 {
        ctx.solana
            .context_mut()
            .await
            .warp_forward_force_reward_interval_end()?;
    }

    user_a.init_lookup_registry().await?;
    let lookup_table = user_a.create_lookup_table().await?;
    // Extend the lookup table with useful addresses
    user_a
        .append_to_lookup_table(
            lookup_table,
            &[
                // Programs
                glow_margin::ID,
                glow_margin_pool::ID,
                glow_metadata::ID,
                anchor_spl::token::ID,
                anchor_spl::associated_token::ID,
                jupiter_cpi::ID,
                // wallet and margin account
                *user_a.address(),
                *user_a.owner(),
                // Mints, pools
                env.usdc.address,
                env.tsol.address,
                env.usdt.address,
                usdc_pool.address,
                usdc_pool.deposit_note_mint,
                usdc_pool.loan_note_mint,
                usdc_pool.vault,
                usdt_pool.address,
                usdt_pool.deposit_note_mint,
                usdt_pool.loan_note_mint,
                usdt_pool.vault,
                tsol_pool.address,
                tsol_pool.deposit_note_mint,
                tsol_pool.loan_note_mint,
                tsol_pool.vault,
                // ATAs
                get_associated_token_address_with_program_id(
                    user_a.address(),
                    &env.usdc.address,
                    &env.usdc.token_program(),
                ),
                get_associated_token_address_with_program_id(
                    user_a.address(),
                    &env.usdt.address,
                    &env.usdt.token_program(),
                ),
                get_associated_token_address_with_program_id(
                    user_a.address(),
                    &env.tsol.address,
                    &env.tsol.token_program(),
                ),
            ],
        )
        .await?;

    user_a.refresh_lookup_tables().await?;

    // Wait for lookup table to become active
    for _ in 0..40 {
        ctx.solana
            .context_mut()
            .await
            .warp_forward_force_reward_interval_end()?;
    }

    // Create some tokens for each user to deposit
    let user_a_tsol_account = ctx
        .tokens()
        .create_account_funded(env.tsol, &wallet_a.pubkey(), 100 * ONE_TSOL)
        .await?;
    let user_b_usdc_account = ctx
        .tokens()
        .create_account_funded(env.usdc, &wallet_b.pubkey(), 50_000 * ONE_USDC)
        .await?;

    // Set the prices for each token
    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &env.usdc.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000,
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *env.usdc_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &env.usdt.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000,
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *env.usdt_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 100 USD +- 1
            &env.tsol.address,
            &TokenPrice {
                exponent: -8,
                price: 10_000_000_000,
                confidence: 100_000_000,
                twap: 10_000_000_000,
                feed_id: *env.tsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    // Deposit user funds into their margin accounts
    let usdc_deposit_amount = 50_000 * ONE_USDC;
    let tsol_deposit_amount = 100 * ONE_TSOL;

    // User A only deposits TSOL
    // User B deposits USDC
    // User A will borrow the USDT from the pool and swap for USDC
    user_a
        .pool_deposit(
            env.tsol,
            Some(user_a_tsol_account),
            TokenChange::shift(tsol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    user_b
        .pool_deposit(
            env.usdc,
            Some(user_b_usdc_account),
            TokenChange::shift(usdc_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // Verify user tokens have been deposited
    assert_eq!(0, ctx.tokens().get_balance(&user_a_tsol_account).await?);
    assert_eq!(0, ctx.tokens().get_balance(&user_b_usdc_account).await?);

    user_a.refresh_all_pool_positions().await?;
    user_b.refresh_all_pool_positions().await?;
    user_a.refresh_all_position_metadata(&refresher).await?;

    let event_authority = Pubkey::from_str("D8cy77BBepLMngZx6ZukaTff5hCt1HrWyKk3Hnd9oitf").unwrap();

    let borrow_dst = env.usdc.associated_token_address(user_a.address());
    let deposit_src = env.usdt.associated_token_address(user_a.address());

    let mut accounts = jupiter_cpi::accounts::Route {
        token_program: anchor_spl::token::ID,
        user_transfer_authority: *user_a.address(),
        user_source_token_account: borrow_dst,
        user_destination_token_account: deposit_src,
        event_authority,
        program: jupiter_cpi::id(),
        destination_token_account: jupiter_cpi::id(),
        destination_mint: env.usdt.address,
        platform_fee_account: jupiter_cpi::id(),
    }
    .to_account_metas(None);

    let saber_accounts = jupiter_cpi::accounts::SaberSwap {
        swap_program: stable_swap_client::ID,
        token_program: anchor_spl::token::ID,
        swap: saber.address,
        swap_authority: saber.authority,
        user_authority: *user_a.address(),
        input_user_account: borrow_dst,
        input_token_account: saber.reserve_b,
        output_token_account: saber.reserve_a,
        output_user_account: deposit_src,
        fees_token_account: saber.admin_fees_a,
    };
    accounts.extend_from_slice(&saber_accounts.to_account_metas(None));

    // This was taken as am example: https://solscan.io/tx/56CWBWyrSti1jCs1jju8C9uZBfXVmatKCxjX7XmiiiSY374CACu1o14zmgkLcrwn58KLndqCurwGYYzojYrcdpYq
    // Jupiter IDL: https://solscan.io/account/JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4#anchorProgramIdl
    // Create CPI swap instruction
    let swap_ix = Instruction {
        program_id: jupiter_cpi::id(),
        accounts,
        data: jupiter_override::Route {
            route_plan: vec![RoutePlanStep {
                swap: jupiter_amm_interface::Swap::Saber,
                percent: 100,
                input_index: 0,
                output_index: 1,
            }],
            in_amount: 19_999 * ONE_USDT,
            quoted_out_amount: Default::default(),
            slippage_bps: 50,
            platform_fee_bps: Default::default(),
        }
        .data(),
    };

    user_a
        .margin_swap(env.usdc, env.usdt, 20_000 * ONE_USDC, &swap_ix)
        .await?;

    // Now the user has 20k USDC loan, ~20k USDT loan and 100 TSOL deposited.
    // To make their account unhealthy, we collapse the price of both TSOL and USDT 😱
    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &env.usdc.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000,
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *env.usdc_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 0.x USD +- 0.01
            &env.usdt.address,
            &TokenPrice {
                exponent: -8,
                price: 80_000_000,
                confidence: 1_000_000,
                twap: 80_000_000,
                feed_id: *env.usdt_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 50 USD +- 1
            &env.tsol.address,
            &TokenPrice {
                exponent: -8,
                price: 5_000_000_000,
                confidence: 100_000_000,
                twap: 5_000_000_000,
                feed_id: *env.tsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    user_a.refresh_all_pool_positions().await?;
    user_b.refresh_all_pool_positions().await?;

    user_a.verify_unhealthy().await?;

    let liquidator = ctx.create_liquidator(10).await.unwrap();

    let user_a_liquidation = ctx.margin_client().liquidator(
        &liquidator,
        user_a.owner(),
        user_a.seed(),
        glow_client::NetworkKind::Localnet,
    )?;

    user_a_liquidation.liquidate_begin(true).await?;

    // The liquidator will reverse the USDC:USDT swap, and take a fee for it.
    let usdt_src = env.usdt.associated_token_address(user_a.address());
    let usdc_dst = env.usdc.associated_token_address(user_a.address());

    let mut accounts = jupiter_cpi::accounts::Route {
        token_program: anchor_spl::token::ID,
        user_transfer_authority: *user_a.address(),
        user_source_token_account: usdt_src,
        user_destination_token_account: usdc_dst,
        event_authority,
        program: jupiter_cpi::id(),
        destination_token_account: jupiter_cpi::id(),
        destination_mint: env.usdc.address,
        platform_fee_account: jupiter_cpi::id(),
    }
    .to_account_metas(None);

    let saber_accounts = jupiter_cpi::accounts::SaberSwap {
        swap_program: stable_swap_client::ID,
        token_program: anchor_spl::token::ID,
        swap: saber.address,
        swap_authority: saber.authority,
        user_authority: *user_a_liquidation.address(),
        input_user_account: usdt_src,
        input_token_account: saber.reserve_a,
        output_token_account: saber.reserve_b,
        output_user_account: usdc_dst,
        fees_token_account: saber.admin_fees_b,
    };
    accounts.extend_from_slice(&saber_accounts.to_account_metas(None));

    let swap_amount = 4_500 * ONE_USDC;
    let swap_ix = Instruction {
        program_id: jupiter_cpi::id(),
        accounts,
        data: jupiter_override::Route {
            route_plan: vec![RoutePlanStep {
                swap: jupiter_amm_interface::Swap::Saber,
                percent: 100,
                input_index: 0,
                output_index: 1,
            }],
            in_amount: swap_amount,
            quoted_out_amount: Default::default(),
            slippage_bps: 50,
            platform_fee_bps: Default::default(),
        }
        .data(),
    };

    user_a_liquidation
        .swap_and_repay(env.usdt, env.usdc, swap_amount, None, &swap_ix, None)
        .await?;

    // TODO: calculate the exact fee and withdraw it
    // user_a_liquidation.collect_liquidation_fees().await?;

    user_a_liquidation.verify_healthy().await?;

    user_a_liquidation.liquidate_end(None).await?;

    // Assert that the liquidator got its share
    user_a.refresh_all_pool_positions().await?;
    user_a.verify_healthy().await?;

    // let liquidator_usdc_fee = env.usdc.associated_token_address(&liquidator.pubkey());
    // assert_eq!(
    //     215_088_706,
    //     ctx.tokens().get_balance(&liquidator_usdc_fee).await?
    // );

    Ok(())
}
