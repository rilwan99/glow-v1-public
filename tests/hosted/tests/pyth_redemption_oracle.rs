#![allow(unused)]
use std::collections::HashMap;

use anyhow::Error;

use glow_instructions::MintInfo;
use glow_margin::{TokenKind, MAX_CLAIM_VALUE_MODIFIER, MAX_COLLATERAL_VALUE_MODIFIER};
use glow_margin_pool::{MarginPoolConfig, PoolFlags, TokenMetadataParams};
use glow_margin_sdk::{
    ix_builder::{MarginPoolConfiguration, MarginPoolIxBuilder},
    solana::transaction::{TransactionBuilderExt, WithSigner},
    tokens::TokenPrice,
    tx_builder::{MarginActionAuthority, TokenDepositsConfig},
};

use glow_program_common::{
    oracle::{pyth_feed_ids::*, TokenPriceOracle},
    token_change::TokenChange,
};
use glow_simulation::assert_custom_program_error;
use solana_sdk::native_token::LAMPORTS_PER_SOL;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use hosted_tests::{
    context::MarginTestContext, margin::MarginPoolSetupInfo, margin_test_context,
    tokens::preset_token_configs::*,
};

const ONE_USDC: u64 = 1_000_000;
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
    ssol: MintInfo,
    tsol: MintInfo,
    ssol_oracle: TokenPriceOracle,
    tsol_oracle: TokenPriceOracle,
}

async fn setup_environment(ctx: &MarginTestContext) -> Result<TestEnv, Error> {
    let (ssol, ssol_oracle) = ctx
        .tokens()
        .create_token_v2(&ssol_config(ctx.payer().pubkey()), 105_000_000, false)
        .await?;
    let (tsol, tsol_oracle) = ctx
        .tokens()
        .create_token_v2(&tsol_config(ctx.payer().pubkey()), 20_000_000_000, false)
        .await?;

    let pools = [
        MarginPoolSetupInfo {
            mint_info: ssol,
            token_kind: TokenKind::Collateral,
            collateral_weight: 95,
            max_leverage: 4_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: ssol_oracle,
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
        tsol,
        ssol,
        tsol_oracle,
        ssol_oracle,
    })
}

// Test redemption rates by directly by querying pool prices of a relevant token.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_redemption_rate() -> Result<(), anyhow::Error> {
    // Get the mocked runtime
    let ctx = margin_test_context!();

    // create position metadata refreshere
    let refresher = ctx.generate_key();
    ctx.margin_config_ix()
        .configure_position_config_refresher(refresher.pubkey(), true)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let env = setup_environment(&ctx).await?;

    // Create our two user wallets, with some SOL funding to get started
    let wallet_a = ctx.create_wallet(10).await?;

    // issue permits for the users
    ctx.issue_permit(wallet_a.pubkey()).await?;

    // Create the user context helpers, which give a simple interface for executing
    // common actions on a margin account
    let user_a = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(Default::default())
        .await?;

    // Create some tokens for each user to deposit
    let user_a_ssol_account = ctx
        .tokens()
        .create_account_funded(env.ssol, &wallet_a.pubkey(), 1_000 * ONE_TSOL)
        .await?;

    // Set the prices for each token
    ctx.tokens()
        .set_price(
            // Set price to 1.02 SOL +- 0.01
            &env.ssol.address,
            &TokenPrice {
                exponent: -8,
                price: 102_000_000,
                confidence: 1_000_000,
                twap: 102_000_000,
                feed_id: *env.ssol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 105 USD +- 1
            &env.tsol.address,
            &TokenPrice {
                exponent: -8,
                price: 10_500_000_000,
                confidence: 100_000_000,
                twap: 10_500_000_000,
                feed_id: *env.tsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    // Deposit user funds into their margin accounts
    const AMOUNT: u64 = 1000;
    let ssol_deposit_amount = AMOUNT * ONE_TSOL;

    user_a
        .pool_deposit(
            env.ssol,
            Some(user_a_ssol_account),
            TokenChange::shift(ssol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // Verify user tokens have been deposited
    assert_eq!(0, ctx.tokens().get_balance(&user_a_ssol_account).await?);

    user_a.refresh_all_pool_positions().await?;

    let positions = user_a.positions().await?;

    // The position should be worth 105 * 1.02 USD
    assert_eq!(positions.len(), 1);
    let position = positions.first().unwrap();
    let expected = 105.0 * 1.02 * AMOUNT as f64;
    let epsilon = 0.000001;
    assert!((position.value().as_f64() - expected).abs() < epsilon);

    Ok(())
}
