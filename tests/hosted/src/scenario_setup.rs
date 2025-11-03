use std::sync::Arc;

use anyhow::Result;
use glow_margin::{AccountFeatureFlags, TokenFeatures};

use crate::{
    context::MarginTestContext,
    margin::MarginUser,
    setup_helper::{borrow_and_dispatch, setup_token, setup_user},
    test_user::TestLiquidator,
};
use glow_instructions::MintInfo;
use glow_margin_sdk::tokens::TokenPrice;
use glow_program_common::oracle::{pyth_feed_ids::*, TokenPriceOracle};
use solana_sdk::native_token::LAMPORTS_PER_SOL;

pub const ONE_USDC: u64 = 1_000_000;
pub const ONE_USDT: u64 = 1_000_000;
pub const ONE_PYUSD: u64 = 1_000_000;
pub const ONE_JUP: u64 = LAMPORTS_PER_SOL;
pub const ONE_TSOL: u64 = LAMPORTS_PER_SOL;
pub const ONE_BTC: u64 = LAMPORTS_PER_SOL;

pub struct Scenario1 {
    pub usdc: MintInfo,
    pub usdc_oracle: TokenPriceOracle,
    pub user_a: MarginUser,
    pub user_b: MarginUser,
    pub liquidator: TestLiquidator,
}

pub struct Scenario2 {
    pub pyusd: MintInfo,
    pub pyusd_oracle: TokenPriceOracle,
    pub user_a: MarginUser,
    pub user_b: MarginUser,
    pub liquidator: TestLiquidator,
}

pub struct Scenario3 {
    pub usdt: MintInfo,
    pub usdt_oracle: TokenPriceOracle,
    pub user_a: MarginUser,
    pub user_b: MarginUser,
    pub liquidator: TestLiquidator,
}

#[macro_export]
macro_rules! scenario1 {
    () => {{
        let ctx = margin_test_context!();
        scenario1_with_ctx(&ctx).await.map(|scen| (ctx, scen))
    }};
}

#[macro_export]
macro_rules! scenario2 {
    () => {{
        let ctx = margin_test_context!();
        scenario2_with_ctx(&ctx).await.map(|scen| (ctx, scen))
    }};
}

#[macro_export]
macro_rules! scenario3 {
    () => {{
        let ctx = margin_test_context!();
        scenario3_with_ctx(&ctx).await.map(|scen| (ctx, scen))
    }};
}

#[allow(clippy::erasing_op)]
pub async fn scenario1_with_ctx(ctx: &Arc<MarginTestContext>) -> Result<Scenario1> {
    // Neither of the tokens in this scenario are Token-2022
    const IS_TOKEN_2022: bool = false;

    const USDC_DECIMALS: u8 = 6;
    const USDC_COLLATERAL_WEIGHT: u16 = 100;
    const USDC_LEVERAGE_MAX: u16 = 400;
    const USDC_PRICE: f64 = 1.0;

    const TSOL_DECIMALS: u8 = 9;
    const TSOL_COLLATERAL_WEIGHT: u16 = 95;
    const TSOL_LEVERAGE_MAX: u16 = 400;
    const TSOL_PRICE: f64 = 100.0;

    let (usdc, usdc_oracle) = setup_token(
        ctx,
        USDC_DECIMALS,
        USDC_COLLATERAL_WEIGHT,
        USDC_LEVERAGE_MAX,
        USDC_PRICE,
        IS_TOKEN_2022,
        usdc_usd(),
        TokenFeatures::default(),
    )
    .await?;
    let (tsol, tsol_oracle) = setup_token(
        ctx,
        TSOL_DECIMALS,
        TSOL_COLLATERAL_WEIGHT,
        TSOL_LEVERAGE_MAX,
        TSOL_PRICE,
        IS_TOKEN_2022,
        sol_usd(),
        TokenFeatures::default(),
    )
    .await?;

    // Create wallet for the liquidator
    let user_a = setup_user(
        ctx,
        vec![(usdc, 5_000_000 * ONE_USDC, 5_000_000 * ONE_USDC)],
        AccountFeatureFlags::default(),
    )
    .await?;
    let user_b = setup_user(
        ctx,
        vec![(tsol, 0, 10_000 * ONE_TSOL)],
        AccountFeatureFlags::default(),
    )
    .await?;

    // Have each user borrow the other's funds
    borrow_and_dispatch(ctx, &user_a, tsol, tsol_oracle, 8000 * ONE_TSOL).await;
    borrow_and_dispatch(ctx, &user_b, usdc, usdc_oracle, 3_500_000 * ONE_USDC).await;

    ctx.tokens()
        .set_price(
            // Set price to 80 USD +- 1
            &tsol.address,
            &TokenPrice {
                exponent: -8,
                price: 8_000_000_000,
                confidence: 100_000_000,
                twap: 8_000_000_000,
                feed_id: *tsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    user_a.user.refresh_all_pool_positions().await?;
    user_b.user.refresh_all_pool_positions().await?;

    Ok(Scenario1 {
        user_a: user_a.user.clone(),
        user_b: user_b.user.clone(),
        usdc,
        usdc_oracle,
        liquidator: TestLiquidator::new(ctx).await?,
    })
}

#[allow(clippy::erasing_op)]
pub async fn scenario2_with_ctx(ctx: &Arc<MarginTestContext>) -> Result<Scenario2> {
    // Both tokens in this scenario are Token-2022
    const IS_TOKEN_2022: bool = true;

    const PYUSD_DECIMALS: u8 = 6;
    const PYUSD_COLLATERAL_WEIGHT: u16 = 100;
    const PYUSD_LEVERAGE_MAX: u16 = 400;
    const PYUSD_PRICE: f64 = 1.0;

    const JUP_DECIMALS: u8 = 9;
    const JUP_COLLATERAL_WEIGHT: u16 = 95;
    const JUP_LEVERAGE_MAX: u16 = 400;
    const JUP_PRICE: f64 = 100.0;

    let (pyusd, pyusd_oracle) = setup_token(
        ctx,
        PYUSD_DECIMALS,
        PYUSD_COLLATERAL_WEIGHT,
        PYUSD_LEVERAGE_MAX,
        PYUSD_PRICE,
        IS_TOKEN_2022,
        pyusd_usd(),
        TokenFeatures::default(),
    )
    .await?;
    let (jup, jup_oracle) = setup_token(
        ctx,
        JUP_DECIMALS,
        JUP_COLLATERAL_WEIGHT,
        JUP_LEVERAGE_MAX,
        JUP_PRICE,
        IS_TOKEN_2022,
        jup_usd(),
        TokenFeatures::default(),
    )
    .await?;

    let user_a = setup_user(
        ctx,
        vec![(pyusd, 5_000_000 * ONE_PYUSD, 5_000_000 * ONE_PYUSD)],
        AccountFeatureFlags::default(),
    )
    .await?;

    let user_b = setup_user(
        ctx,
        vec![(jup, 0, 10_000 * ONE_JUP)],
        AccountFeatureFlags::default(),
    )
    .await?;

    // Have each user borrow the other's funds
    borrow_and_dispatch(ctx, &user_a, jup, jup_oracle, 8_000 * ONE_JUP).await;
    borrow_and_dispatch(ctx, &user_b, pyusd, pyusd_oracle, 3_500_000 * ONE_PYUSD).await;

    ctx.tokens()
        .set_price(
            // Set price to 80 USD +- 1
            &jup.address,
            &TokenPrice {
                exponent: -8,
                price: 8_000_000_000,
                confidence: 100_000_000,
                twap: 8_000_000_000,
                feed_id: *jup_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    // Refresh all pool positions for both users
    user_a.user.refresh_all_pool_positions().await?;
    user_b.user.refresh_all_pool_positions().await?;

    Ok(Scenario2 {
        user_a: user_a.user.clone(),
        user_b: user_b.user.clone(),
        pyusd,
        pyusd_oracle,
        liquidator: TestLiquidator::new(ctx).await?,
    })
}

#[allow(clippy::erasing_op)]
pub async fn scenario3_with_ctx(ctx: &Arc<MarginTestContext>) -> Result<Scenario3> {
    const TOKEN_2022: bool = true;

    const USDT_DECIMALS: u8 = 6;
    const USDT_COLLATERAL_WEIGHT: u16 = 100;
    const USDT_LEVERAGE_MAX: u16 = 400;
    const USDT_PRICE: f64 = 1.0;

    const BTC_DECIMALS: u8 = 9;
    const BTC_COLLATERAL_WEIGHT: u16 = 100;
    const BTC_LEVERAGE_MAX: u16 = 400;
    const BTC_PRICE: f64 = 100_000.0;

    let (btc, btc_oracle) = setup_token(
        ctx,
        BTC_DECIMALS,
        BTC_COLLATERAL_WEIGHT,
        BTC_LEVERAGE_MAX,
        BTC_PRICE,
        TOKEN_2022, // Token-2022
        btc_usd(),
        TokenFeatures::default(),
    )
    .await?;
    let (usdt, usdt_oracle) = setup_token(
        ctx,
        USDT_DECIMALS,
        USDT_COLLATERAL_WEIGHT,
        USDT_LEVERAGE_MAX,
        USDT_PRICE,
        !TOKEN_2022, // Not Token-2022
        usdt_usd(),
        TokenFeatures::default(),
    )
    .await?;

    let user_a = setup_user(
        ctx,
        vec![(usdt, 5_000_000 * ONE_USDT, 5_000_000 * ONE_USDT)],
        AccountFeatureFlags::default(),
    )
    .await?;
    let user_b = setup_user(
        ctx,
        vec![(btc, 10 * ONE_BTC, 10 * ONE_BTC)],
        AccountFeatureFlags::default(),
    )
    .await?;

    // Have each user borrow the other's funds
    borrow_and_dispatch(ctx, &user_a, btc, btc_oracle, 8 * ONE_BTC).await;
    borrow_and_dispatch(ctx, &user_b, usdt, usdt_oracle, 3_500_000 * ONE_USDT).await;

    ctx.tokens()
        .set_price(
            // Set price to 80,000 USD +- 1
            &btc.address,
            &TokenPrice {
                exponent: -9,
                price: 80_000_000_000_000,
                confidence: 1_000_000_000,
                twap: 80_000_000_000_000,
                feed_id: *btc_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    // Refresh all pool positions for both users
    user_a.user.refresh_all_pool_positions().await?;
    user_b.user.refresh_all_pool_positions().await?;

    Ok(Scenario3 {
        user_a: user_a.user.clone(),
        user_b: user_b.user.clone(),
        usdt,
        usdt_oracle,
        liquidator: TestLiquidator::new(ctx).await?,
    })
}
