use anyhow::Result;

use glow_margin::{ErrorCode, TokenKind};
use glow_program_common::{oracle::pyth_feed_ids::*, token_change::TokenChange};
use glow_simulation::assert_custom_program_error;
use hosted_tests::{
    margin::MarginPoolSetupInfo,
    margin_test_context, scenario1, scenario2, scenario3,
    scenario_setup::{
        scenario1_with_ctx, scenario2_with_ctx, scenario3_with_ctx, ONE_PYUSD, ONE_USDC, ONE_USDT,
    },
    setup_helper::{create_token_with_pyth, DEFAULT_POOL_CONFIG},
};
use solana_sdk::signature::Signer;

/// Verify that creating a margin pool works.
/// This is used as part of the setup process for all the
/// liquidation tests below.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn empty_margin_pool_creation() -> Result<()> {
    let ctx = margin_test_context!();
    let token_manager = ctx.tokens();
    let feed_id = usdc_usd();
    let (mint_info, _) = create_token_with_pyth(
        &token_manager,
        ctx.mint_authority().pubkey(),
        ctx.oracle_authority().pubkey(),
        100.0,
        9,
        true,
        feed_id,
    )
    .await?;

    let margin_client = ctx.margin_client();
    margin_client.create_empty_margin_pool(mint_info).await?;

    Ok(())
}

/// Verify that configuring a margin pool works.
/// This is used as part of the setup process for all the
/// liquidation tests below.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn create_and_configure_margin_pool() -> Result<()> {
    let ctx = margin_test_context!();
    let token_manager = ctx.tokens();
    let feed_id = usdc_usd();
    let (mint_info, token_oracle) = create_token_with_pyth(
        &token_manager,
        ctx.mint_authority().pubkey(),
        ctx.oracle_authority().pubkey(),
        100.0,
        9,
        true,
        feed_id,
    )
    .await?;

    let setup = MarginPoolSetupInfo {
        mint_info,
        collateral_weight: 95,
        max_leverage: 100,
        token_kind: TokenKind::Collateral,
        config: DEFAULT_POOL_CONFIG,
        oracle: token_oracle,
        max_staleness: 30,
        token_features: Default::default(),
    };

    let margin_client = ctx.margin_client();
    margin_client.create_pool(&setup).await?;

    Ok(())
}

// Main test cases.
// Scenario 1: two SPL standard tokens
// Scenario 2: two Token-2022 tokens
// Scenario 3: one standard and one Token-2022 token
//
/// Account liquidations
///
/// This test creates 2 users who deposit collateral and take loans in the
/// margin account. The price of the loan token moves adversely, leading to
/// liquidations. One user borrowed conservatively, and is not subject to
/// liquidation, while the other user gets liquidated.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn cannot_liquidate_healthy_user() -> Result<()> {
    let scen1 = scenario1!()?.1;
    let scen2 = scenario2!()?.1;
    let scen3 = scenario3!()?.1;

    // A liquidator tries to liquidate User A, it should not be able to
    let result = scen1.liquidator.begin(&scen1.user_a, true).await;
    assert_custom_program_error(ErrorCode::Healthy, result);
    let result = scen2.liquidator.begin(&scen2.user_a, true).await;
    assert_custom_program_error(ErrorCode::Healthy, result);
    let result = scen3.liquidator.begin(&scen3.user_a, true).await;
    assert_custom_program_error(ErrorCode::Healthy, result);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn cannot_end_nonexistent_liquidation() -> Result<()> {
    let scen1 = scenario1!()?.1;
    let scen2 = scenario2!()?.1;
    let scen3 = scenario3!()?.1;

    // A liquidator should not be able to end liquidation of an account that is
    // not being liquidated
    let result = scen1
        .liquidator
        .for_user(&scen1.user_a)
        .unwrap()
        .liquidate_end(None)
        .await;
    assert!(result.is_err());

    let result = scen2
        .liquidator
        .for_user(&scen2.user_a)
        .unwrap()
        .liquidate_end(None)
        .await;
    assert!(result.is_err());

    let result = scen3
        .liquidator
        .for_user(&scen3.user_a)
        .unwrap()
        .liquidate_end(None)
        .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn cannot_transact_when_being_liquidated() -> Result<()> {
    let scen1 = scenario1!().unwrap().1;
    let scen2 = scenario2!().unwrap().1;
    let scen3 = scenario3!().unwrap().1;

    // A liquidator tries to liquidate User B, it should be able to
    scen1.liquidator.begin(&scen1.user_b, false).await.unwrap();
    scen2.liquidator.begin(&scen2.user_b, false).await.unwrap();
    scen3.liquidator.begin(&scen3.user_b, false).await.unwrap();

    // When User B is being liquidated, they should be unable to transact
    let result = scen1
        .user_b
        .margin_repay(scen1.usdc, TokenChange::shift(1_000_000 * ONE_USDC))
        .await;
    assert_custom_program_error(ErrorCode::Liquidating, result);

    let result = scen2
        .user_b
        .margin_repay(scen2.pyusd, TokenChange::shift(1_000_000 * ONE_PYUSD))
        .await;
    assert_custom_program_error(ErrorCode::Liquidating, result);

    let result = scen3
        .user_b
        .margin_repay(scen3.usdt, TokenChange::shift(1_000_000 * ONE_USDT))
        .await;
    assert_custom_program_error(ErrorCode::Liquidating, result);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn liquidator_can_repay_from_unhealthy_to_healthy_state() -> Result<()> {
    let scen1 = scenario1!().unwrap().1;
    let scen2 = scenario2!().unwrap().1;
    let scen3 = scenario3!().unwrap().1;

    let liq1 = scen1.liquidator.begin(&scen1.user_b, true).await.unwrap();
    liq1.verify_unhealthy().await.unwrap();

    let liq2 = scen2.liquidator.begin(&scen2.user_b, true).await.unwrap();
    liq2.verify_unhealthy().await.unwrap();

    let liq3 = scen3.liquidator.begin(&scen3.user_b, true).await.unwrap();
    liq3.verify_unhealthy().await.unwrap();

    // Execute a repayment on behalf of the user
    liq1.margin_repay(scen1.usdc, 500_000 * ONE_USDC)
        .await
        .unwrap();

    scen1.user_b.verify_healthy().await.unwrap();

    // Execute a repayment on behalf of the user
    liq2.margin_repay(scen2.pyusd, 500_000 * ONE_PYUSD)
        .await
        .unwrap();

    scen2.user_b.verify_healthy().await.unwrap();

    liq3.margin_repay(scen3.usdt, 500_000 * ONE_USDT)
        .await
        .unwrap();

    scen3.user_b.verify_healthy().await.unwrap();

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn liquidator_can_end_liquidation_when_unhealthy() -> Result<()> {
    let scen1 = scenario1!().unwrap().1;
    let scen2 = scenario2!().unwrap().1;
    let scen3 = scenario3!().unwrap().1;

    let liq1 = scen1.liquidator.begin(&scen1.user_b, true).await.unwrap();
    liq1.verify_unhealthy().await.unwrap();
    liq1.liquidate_end(None).await.unwrap();

    let liq2 = scen2.liquidator.begin(&scen2.user_b, true).await.unwrap();
    liq2.verify_unhealthy().await.unwrap();
    liq2.liquidate_end(None).await.unwrap();

    let liq3 = scen3.liquidator.begin(&scen3.user_b, true).await.unwrap();
    liq3.verify_unhealthy().await.unwrap();
    liq3.liquidate_end(None).await.unwrap();

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn no_one_else_can_liquidate_after_liquidate_begin() -> Result<()> {
    let (ctx, scen1) = scenario1!().unwrap();

    // A liquidator tries to liquidate User B, it should be able to
    scen1.liquidator.begin(&scen1.user_b, false).await.unwrap();

    // If an account is still being liquidated, another liquidator should not
    // be able to begin or stop liquidating it
    let rogue_liquidator = ctx.create_liquidator(100).await.unwrap();
    let user_b_rliq = ctx
        .margin_client()
        .liquidator(
            &rogue_liquidator,
            scen1.user_b.owner(),
            scen1.user_b.seed(),
            glow_client::NetworkKind::Localnet,
        )
        .unwrap();

    // Should fail to begin liquidation
    assert_custom_program_error(
        ErrorCode::Liquidating,
        user_b_rliq.liquidate_begin(true).await,
    );

    let (ctx, scen2) = scenario2!().unwrap();

    scen2.liquidator.begin(&scen2.user_b, false).await.unwrap();

    let rogue_liquidator2 = ctx.create_liquidator(100).await.unwrap();
    let user_b_rliq2 = ctx
        .margin_client()
        .liquidator(
            &rogue_liquidator2,
            scen2.user_b.owner(),
            scen2.user_b.seed(),
            glow_client::NetworkKind::Localnet,
        )
        .unwrap();

    // Should fail to begin liquidation
    assert_custom_program_error(
        ErrorCode::Liquidating,
        user_b_rliq2.liquidate_begin(true).await,
    );

    let (ctx, scen3) = scenario3!().unwrap();

    scen3.liquidator.begin(&scen3.user_b, false).await.unwrap();

    let rogue_liquidator3 = ctx.create_liquidator(100).await.unwrap();
    let user_b_rliq3 = ctx
        .margin_client()
        .liquidator(
            &rogue_liquidator3,
            scen3.user_b.owner(),
            scen3.user_b.seed(),
            glow_client::NetworkKind::Localnet,
        )
        .unwrap();

    // Should fail to begin liquidation
    assert_custom_program_error(
        ErrorCode::Liquidating,
        user_b_rliq3.liquidate_begin(true).await,
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn liquidation_completes() -> Result<()> {
    let scen1 = scenario1!().unwrap().1;

    // A liquidator tries to liquidate User B, it should be able to
    let user_b_liq = scen1.liquidator.begin(&scen1.user_b, false).await.unwrap();

    // Execute a repayment on behalf of the user
    user_b_liq
        .margin_repay(scen1.usdc, 500_000 * ONE_USDC)
        .await
        .unwrap();

    // The liquidator should be able to end liquidation after liquidating
    user_b_liq.liquidate_end(None).await.unwrap();

    // User B should now be able to transact again
    scen1
        .user_b
        .margin_repay(scen1.usdc, TokenChange::shift(200_000 * ONE_USDC))
        .await
        .unwrap();

    let scen2 = scenario2!().unwrap().1;

    let user_b_liq2 = scen2.liquidator.begin(&scen2.user_b, false).await.unwrap();

    // Execute a repayment on behalf of the user
    user_b_liq2
        .margin_repay(scen2.pyusd, 500_000 * ONE_PYUSD)
        .await
        .unwrap();

    // The liquidator should be able to end liquidation after liquidating
    user_b_liq2.liquidate_end(None).await.unwrap();

    // User B should now be able to transact again
    scen2
        .user_b
        .margin_repay(scen2.pyusd, TokenChange::shift(200_000 * ONE_PYUSD))
        .await
        .unwrap();

    let scen3 = scenario3!().unwrap().1;

    let user_b_liq3 = scen3.liquidator.begin(&scen3.user_b, true).await.unwrap();

    user_b_liq3
        .margin_repay(scen3.usdt, 500_000 * ONE_USDT)
        .await
        .unwrap();

    user_b_liq3.liquidate_end(None).await.unwrap();

    scen3
        .user_b
        .margin_repay(scen3.usdt, TokenChange::shift(200_000 * ONE_USDT))
        .await
        .unwrap();

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn cannot_withdraw_too_much_during_liquidation() -> Result<()> {
    let scen1 = scenario1!().unwrap().1;

    let user_b_liq = scen1.liquidator.begin(&scen1.user_b, true).await.unwrap();

    user_b_liq
        .margin_repay(scen1.usdc, 500_000 * ONE_USDC)
        .await
        .unwrap();

    user_b_liq.verify_healthy().await.unwrap();

    let result = user_b_liq.withdraw(scen1.usdc, 200_000 * ONE_USDC).await;

    assert_custom_program_error(ErrorCode::LiquidationLostValue, result);

    let scen2 = scenario2!().unwrap().1;

    let user_b_liq2 = scen2.liquidator.begin(&scen2.user_b, true).await.unwrap();

    user_b_liq2
        .margin_repay(scen2.pyusd, 500_000 * ONE_PYUSD)
        .await
        .unwrap();

    user_b_liq2.verify_healthy().await.unwrap();

    let result = user_b_liq2.withdraw(scen2.pyusd, 200_000 * ONE_PYUSD).await;

    assert_custom_program_error(ErrorCode::LiquidationLostValue, result);

    let scen3 = scenario3!().unwrap().1;

    let user_b_liq3 = scen3.liquidator.begin(&scen3.user_b, true).await.unwrap();

    user_b_liq3
        .margin_repay(scen3.usdt, 500_000 * ONE_USDT)
        .await
        .unwrap();

    user_b_liq3.verify_healthy().await.unwrap();

    let result = user_b_liq3.withdraw(scen3.usdt, 200_000 * ONE_USDT).await;

    assert_custom_program_error(ErrorCode::LiquidationLostValue, result);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn cannot_withdraw_without_repaying_during_liquidation() -> Result<()> {
    let scen1 = scenario1!().unwrap().1;

    let user_b_liq = scen1.liquidator.begin(&scen1.user_b, true).await.unwrap();
    let result = user_b_liq.withdraw(scen1.usdc, 40 * ONE_USDC).await;

    assert_custom_program_error(ErrorCode::LiquidationLostValue, result);

    let scen2 = scenario2!().unwrap().1;

    let user_b_liq2 = scen2.liquidator.begin(&scen2.user_b, true).await.unwrap();

    let result = user_b_liq2.withdraw(scen2.pyusd, 40 * ONE_PYUSD).await;

    assert_custom_program_error(ErrorCode::LiquidationLostValue, result);

    let scen3 = scenario3!().unwrap().1;

    let user_b_liq3 = scen3.liquidator.begin(&scen3.user_b, true).await.unwrap();

    let result = user_b_liq3.withdraw(scen3.usdt, 200_000 * ONE_USDT).await;

    assert_custom_program_error(ErrorCode::LiquidationLostValue, result);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn cannot_borrow_during_liquidation() -> Result<()> {
    let scen1 = scenario1!().unwrap().1;

    let user_b_liq = scen1.liquidator.begin(&scen1.user_b, false).await.unwrap();

    // Cannot only borrow
    let result = user_b_liq
        .borrow(scen1.usdc, scen1.usdc_oracle, 10 * ONE_USDC)
        .await;
    assert_custom_program_error(ErrorCode::LiquidationLostValue, result);

    let scen2 = scenario2!().unwrap().1;

    let user_b_liq2 = scen2.liquidator.begin(&scen2.user_b, true).await.unwrap();

    let result = user_b_liq2
        .borrow(scen2.pyusd, scen2.pyusd_oracle, 10 * ONE_PYUSD)
        .await;

    assert_custom_program_error(ErrorCode::LiquidationLostValue, result);

    let scen3 = scenario3!().unwrap().1;

    let user_b_liq3 = scen3.liquidator.begin(&scen3.user_b, true).await.unwrap();

    let result = user_b_liq3
        .borrow(scen3.usdt, scen3.usdt_oracle, 10 * ONE_USDT)
        .await;

    assert_custom_program_error(ErrorCode::LiquidationLostValue, result);

    Ok(())
}

/// The owner is provided as the authority and signs
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn owner_cannot_end_liquidation_before_timeout() -> Result<()> {
    let scen1 = scenario1!().unwrap().1;

    scen1.liquidator.begin(&scen1.user_b, false).await.unwrap();

    let result = scen1
        .user_b
        .liquidate_end(Some(scen1.liquidator.wallet.pubkey()))
        .await;
    assert_custom_program_error(ErrorCode::UnauthorizedLiquidator, result);

    let scen2 = scenario2!().unwrap().1;

    scen2.liquidator.begin(&scen2.user_b, false).await.unwrap();

    let result = scen2
        .user_b
        .liquidate_end(Some(scen2.liquidator.wallet.pubkey()))
        .await;
    assert_custom_program_error(ErrorCode::UnauthorizedLiquidator, result);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
// #[cfg_attr(feature = "localnet", ignore = "does not run on localnet")]
async fn owner_can_end_liquidation_after_timeout() -> Result<()> {
    let (ctx, scen1) = scenario1!().unwrap();

    scen1.liquidator.begin(&scen1.user_b, false).await.unwrap();

    let mut clock = ctx.rpc().get_clock().await.unwrap();
    clock.unix_timestamp += 61;
    ctx.rpc().set_clock(clock).await.unwrap();

    scen1
        .user_b
        .liquidate_end(Some(scen1.liquidator.wallet.pubkey()))
        .await
        .unwrap();

    let (ctx, scen2) = scenario2!().unwrap();

    scen2.liquidator.begin(&scen2.user_b, false).await.unwrap();

    let mut clock = ctx.rpc().get_clock().await.unwrap();
    clock.unix_timestamp += 61;
    ctx.rpc().set_clock(clock).await.unwrap();

    scen2
        .user_b
        .liquidate_end(Some(scen2.liquidator.wallet.pubkey()))
        .await
        .unwrap();

    let (ctx, scen3) = scenario3!().unwrap();

    scen3.liquidator.begin(&scen3.user_b, false).await.unwrap();

    let mut clock = ctx.rpc().get_clock().await.unwrap();
    clock.unix_timestamp += 61;
    ctx.rpc().set_clock(clock).await.unwrap();

    scen3
        .user_b
        .liquidate_end(Some(scen3.liquidator.wallet.pubkey()))
        .await
        .unwrap();

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn liquidator_permission_is_removable() -> Result<()> {
    let (ctx, scen1) = scenario1!().unwrap();

    ctx.margin_client()
        .set_liquidator_metadata(scen1.liquidator.wallet.pubkey(), false)
        .await
        .unwrap();

    // A liquidator tries to liquidate User B, it should no longer have authority to do that
    let result = scen1.liquidator.begin(&scen1.user_b, false).await;

    assert_custom_program_error(anchor_lang::error::ErrorCode::AccountNotInitialized, result);

    let (ctx, scen2) = scenario2!().unwrap();

    ctx.margin_client()
        .set_liquidator_metadata(scen2.liquidator.wallet.pubkey(), false)
        .await
        .unwrap();

    let result = scen2.liquidator.begin(&scen2.user_b, false).await;

    assert_custom_program_error(anchor_lang::error::ErrorCode::AccountNotInitialized, result);

    let (ctx, scen3) = scenario3!().unwrap();

    ctx.margin_client()
        .set_liquidator_metadata(scen3.liquidator.wallet.pubkey(), false)
        .await
        .unwrap();

    let result = scen3.liquidator.begin(&scen3.user_b, false).await;

    assert_custom_program_error(anchor_lang::error::ErrorCode::AccountNotInitialized, result);

    Ok(())
}
