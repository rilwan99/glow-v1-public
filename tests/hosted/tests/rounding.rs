use anyhow::{Error, Result};

use glow_instructions::MintInfo;
use glow_margin_sdk::{tokens::TokenPrice, tx_builder::MarginActionAuthority};
use glow_program_common::{oracle::TokenPriceOracle, token_change::TokenChange};
use glow_simulation::assert_custom_program_error;
use solana_sdk::clock::Clock;
use solana_sdk::native_token::LAMPORTS_PER_SOL;
use solana_sdk::signature::Signer;

use hosted_tests::{
    context::MarginTestContext, margin::MarginPoolSetupInfo, margin_test_context,
    tokens::preset_token_configs::*,
};

use glow_margin::{AccountFeatureFlags, TokenKind};
use glow_margin_pool::{MarginPoolConfig, PoolFlags};

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
    usdc: MintInfo,
    tsol: MintInfo,
    usdc_oracle: TokenPriceOracle,
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
        ctx.margin_client().create_pool(&pool_info).await?;
    }

    Ok(TestEnv {
        usdc,
        tsol,
        usdc_oracle,
        tsol_oracle,
    })
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn rounding_poc() -> Result<()> {
    let ctx = margin_test_context!();
    let env = setup_environment(&ctx).await.unwrap();

    let wallet_a = ctx.create_wallet(10).await.unwrap();
    let wallet_b = ctx.create_wallet(10).await.unwrap();
    let wallet_c = ctx.create_wallet(10).await.unwrap();

    // issue permits for the users
    ctx.issue_permit(wallet_a.pubkey()).await?;
    ctx.issue_permit(wallet_b.pubkey()).await?;
    ctx.issue_permit(wallet_c.pubkey()).await?;

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
    let user_c = ctx
        .margin_client()
        .user(&wallet_c, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::default())
        .await?;

    let user_a_usdc_account = ctx
        .tokens()
        .create_account_funded(env.usdc, &wallet_a.pubkey(), 10_000_000 * ONE_USDC)
        .await
        .unwrap();
    let user_b_tsol_account = ctx
        .tokens()
        .create_account_funded(env.tsol, &wallet_b.pubkey(), 10_000 * ONE_TSOL)
        .await
        .unwrap();
    let user_c_usdc_account = ctx
        .tokens()
        .create_account_funded(env.usdc, &wallet_c.pubkey(), 0)
        .await
        .unwrap();

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
        .await
        .unwrap();
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
        .await
        .unwrap();

    user_a
        .pool_deposit(
            env.usdc,
            Some(user_a_usdc_account),
            TokenChange::shift(5_000_000 * ONE_USDC),
            MarginActionAuthority::AccountAuthority,
        )
        .await
        .unwrap();
    user_b
        .pool_deposit(
            env.tsol,
            Some(user_b_tsol_account),
            TokenChange::shift(10_000 * ONE_TSOL),
            MarginActionAuthority::AccountAuthority,
        )
        .await
        .unwrap();

    user_a.refresh_all_pool_positions().await.unwrap();
    user_b.refresh_all_pool_positions().await.unwrap();

    user_b
        .borrow(env.usdc, TokenChange::shift(50000000000))
        .await
        .unwrap();

    let mut clk: Clock = match ctx.rpc().get_clock().await {
        Ok(c) => c,
        _ => panic!("bad"),
    };

    // 1 second later...
    clk.unix_timestamp += 1;
    ctx.rpc().set_clock(clk).await.unwrap();

    user_a.refresh_all_pool_positions().await.unwrap();
    user_b.refresh_all_pool_positions().await.unwrap();

    // If the rounding is performed correctly, the user should try to burn 1 note,
    // and this should fail as they have no notes to burn.
    let withdraw_result = user_c
        .withdraw(env.usdc, &user_c_usdc_account, TokenChange::shift(1))
        .await;

    // Should not succeed, there should be insufficient funds to burn notes
    assert_custom_program_error(
        anchor_spl::token::spl_token::error::TokenError::InsufficientFunds as u32,
        withdraw_result,
    );

    Ok(())
}
