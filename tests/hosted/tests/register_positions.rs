#![allow(unused)]
use std::collections::HashMap;

use anchor_lang::{InstructionData, ToAccountMetas};
use anyhow::Error;
use glow_environment::config::TokenDescription;
use glow_instructions::{margin::derive_token_config, MintInfo};
use glow_margin::{
    AccountFeatureFlags, TokenConfig, TokenConfigUpdate, TokenFeatures, TokenKind,
    MAX_CLAIM_VALUE_MODIFIER, MAX_COLLATERAL_VALUE_MODIFIER,
};
use glow_margin_pool::{MarginPoolConfig, PoolFlags, TokenMetadataParams};
use glow_margin_sdk::{
    get_state::get_anchor_account,
    ix_builder::{MarginPoolConfiguration, MarginPoolIxBuilder},
    solana::transaction::{TransactionBuilderExt, WithSigner},
    tokens::TokenPrice,
    tx_builder::{MarginActionAuthority, TokenDepositsConfig},
};

use glow_program_common::oracle::{pyth_feed_ids::*, TokenPriceOracle};
use glow_simulation::{assert_custom_program_error, send_and_confirm};
use glow_test_service::TokenCreateParams;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use solana_sdk::{instruction::Instruction, native_token::LAMPORTS_PER_SOL};

use hosted_tests::{
    context::{MarginTestContext, TestContextSetupInfo},
    margin::MarginPoolSetupInfo,
    margin_test_context,
    test_positions::{
        close_test_adapter_position, create_test_service_authority, register_test_adapter_position,
    },
    tokens::preset_token_configs::*,
};

const ONE_TSOL: u64 = LAMPORTS_PER_SOL;
const ONE_RUSD: u64 = 1_000_000;

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
    tsol: MintInfo,
    tsol_oracle: TokenPriceOracle,
}

async fn setup_environment(ctx: &MarginTestContext) -> Result<TestEnv, Error> {
    let authority = ctx.payer().pubkey();
    let (tsol, tsol_oracle) = ctx
        .tokens()
        .create_token_v2(&tsol_config(authority), 20_000_000_000, false)
        .await?;

    let pools = [MarginPoolSetupInfo {
        mint_info: tsol,
        token_kind: TokenKind::AdapterCollateral,
        collateral_weight: 95,
        max_leverage: 4_00,
        config: DEFAULT_POOL_CONFIG,
        oracle: tsol_oracle,
        max_staleness: 30,
        token_features: TokenFeatures::default(),
    }];

    for pool_info in pools {
        ctx.margin_client()
            .configure_token_deposits(
                pool_info.mint_info,
                Some(&TokenDepositsConfig {
                    oracle: pool_info.oracle,
                    collateral_weight: pool_info.collateral_weight,
                    max_staleness: 30,
                    token_features: pool_info.token_features,
                }),
            )
            .await?;
        ctx.margin_client().create_pool(&pool_info).await?;
    }

    Ok(TestEnv { tsol, tsol_oracle })
}

/// Test registering positions
///
/// We want to test that all kinds of positions can be registered. These are:
/// - Collateral
/// - AdapterCollateral
/// - Claim
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_register_positions() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("test_register_positions");

    let authority = ctx.payer();

    let env = setup_environment(&ctx).await?;

    create_test_service_authority(&ctx.rpc(), authority).await?;

    ctx.margin_client()
        .register_adapter(&glow_test_service::ID)
        .await?;

    let wallet_a = ctx.create_wallet(10).await?;
    ctx.issue_permit(wallet_a.pubkey()).await?;

    let user_a = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(Default::default())
        .await?;

    // Margin user should be able to register a margin token
    user_a.create_deposit_position(env.tsol).await?;

    let (token_a, token_a_oracle) = ctx
        .tokens()
        .create_token_v2(
            &TokenCreateParams {
                symbol: "A".to_string(),
                name: "A".to_string(),
                decimals: 6,
                authority: authority.pubkey(),
                oracle_authority: authority.pubkey(),
                max_amount: u64::MAX,
                source_symbol: "USDC".to_string(),
                price_ratio: 1.0,
                price_oracle: TokenPriceOracle::PythPull {
                    feed_id: usdc_usd(),
                },
            },
            1_000_000,
            false,
        )
        .await?;

    let (token_b, token_b_oracle) = ctx
        .tokens()
        .create_token_v2(
            &TokenCreateParams {
                symbol: "B".to_string(),
                name: "B".to_string(),
                decimals: 6,
                authority: authority.pubkey(),
                oracle_authority: authority.pubkey(),
                max_amount: u64::MAX,
                source_symbol: "USDC".to_string(),
                price_ratio: 1.0,
                price_oracle: TokenPriceOracle::PythPull {
                    feed_id: usdc_usd(),
                },
            },
            1_000_000,
            false,
        )
        .await?;

    let (token_c, token_c_oracle) = ctx
        .tokens()
        .create_token_v2(
            &TokenCreateParams {
                symbol: "C".to_string(),
                name: "C".to_string(),
                decimals: 6,
                authority: authority.pubkey(),
                oracle_authority: authority.pubkey(),
                max_amount: u64::MAX,
                source_symbol: "USDC".to_string(),
                price_ratio: 1.0,
                price_oracle: TokenPriceOracle::PythPull {
                    feed_id: usdc_usd(),
                },
            },
            1_000_000,
            false,
        )
        .await?;

    // Configure a token owned by an adapter
    let ix = ctx.margin_config_ix().configure_token(
        token_a.address,
        TokenConfigUpdate {
            underlying_mint: token_a.address,
            underlying_mint_token_program: token_a.token_program(),
            admin: glow_margin::TokenAdmin::Adapter(glow_margin_pool::ID),
            token_kind: TokenKind::Collateral,
            value_modifier: 100,
            max_staleness: 0,
            token_features: Default::default(),
        },
    );
    send_and_confirm(&ctx.rpc(), &[ix], &[&ctx.airspace_authority]).await?;

    // We should successfully register an adapter collateral
    let ix = ctx.margin_config_ix().configure_token(
        token_b.address,
        TokenConfigUpdate {
            underlying_mint: token_b.address,
            underlying_mint_token_program: token_a.token_program(),
            admin: glow_margin::TokenAdmin::Adapter(glow_test_service::ID),
            token_kind: TokenKind::AdapterCollateral,
            value_modifier: 100,
            max_staleness: 0,
            token_features: Default::default(),
        },
    );
    send_and_confirm(&ctx.rpc(), &[ix], &[&ctx.airspace_authority]).await?;

    // Configure a token owned by the margin account
    // An adapter collateral should not be registerable as owned by the margin admin
    for token_kind in [TokenKind::AdapterCollateral, TokenKind::Claim] {
        let ix = ctx.margin_config_ix().configure_token(
            token_b.address,
            TokenConfigUpdate {
                underlying_mint: token_b.address,
                underlying_mint_token_program: token_b.token_program(),
                admin: glow_margin::TokenAdmin::Margin {
                    oracle: TokenPriceOracle::PythPull {
                        feed_id: usdc_usd(),
                    },
                },
                token_kind,
                value_modifier: 100,
                max_staleness: 0,
                token_features: Default::default(),
            },
        );
        let result = send_and_confirm(&ctx.rpc(), &[ix], &[&ctx.airspace_authority]).await;
        assert_custom_program_error(glow_margin::ErrorCode::InvalidConfigTokenKind, result);
    }

    // With token B registered as an adapter collateral, only the test service adapter should be able to register the position
    register_test_adapter_position(
        &ctx.rpc(),
        &user_a.signer,
        ctx.airspace_details.address,
        *user_a.address(),
        token_b,
    )
    .await?;

    // Verify that the position has been registered
    let positions = user_a.positions().await?;
    let token_b_position = positions
        .iter()
        .find(|p| p.token == token_b.address)
        .unwrap();
    assert_eq!(token_b_position.adapter, glow_test_service::ID);
    assert_eq!(token_b_position.kind(), TokenKind::AdapterCollateral);

    // Now register another adapter collateral owned by another program. The test service should not be able to
    // register a position of another adapter.
    let (token_d, token_d_oracle) = ctx
        .tokens()
        .create_token_v2(
            &TokenCreateParams {
                symbol: "D".to_string(),
                name: "D".to_string(),
                decimals: 6,
                authority: authority.pubkey(),
                oracle_authority: authority.pubkey(),
                max_amount: u64::MAX,
                source_symbol: "USDC".to_string(),
                price_ratio: 1.0,
                price_oracle: TokenPriceOracle::PythPull {
                    feed_id: usdc_usd(),
                },
            },
            1_000_000,
            false,
        )
        .await?;

    let ix = ctx.margin_config_ix().configure_token(
        token_d.address,
        TokenConfigUpdate {
            underlying_mint: token_d.address,
            underlying_mint_token_program: token_d.token_program(),
            admin: glow_margin::TokenAdmin::Adapter(glow_margin_pool::ID),
            token_kind: TokenKind::Collateral,
            value_modifier: 100,
            max_staleness: 0,
            token_features: Default::default(),
        },
    );
    send_and_confirm(&ctx.rpc(), &[ix], &[&ctx.airspace_authority]).await?;

    let result = register_test_adapter_position(
        &ctx.rpc(),
        &user_a.signer,
        ctx.airspace_details.address,
        *user_a.address(),
        token_d,
    )
    .await;
    // Raw constraint violated because the token_config doesn't meet seed constraints.
    assert_eq!(
        result.err().unwrap().to_string().as_str(),
        "transport transaction error: Error processing Instruction 0: custom program error: 0x7d3"
    );

    // Should be able to close Token B's position
    close_test_adapter_position(
        &ctx.rpc(),
        &user_a.signer,
        ctx.airspace_details.address,
        *user_a.address(),
        token_b,
    )
    .await?;

    return Ok(());

    let ix = ctx.margin_config_ix().configure_token(
        token_b.address,
        TokenConfigUpdate {
            underlying_mint: token_b.address,
            underlying_mint_token_program: token_b.token_program(),
            admin: glow_margin::TokenAdmin::Margin {
                oracle: TokenPriceOracle::PythPull {
                    feed_id: usdc_usd(),
                },
            },
            token_kind: TokenKind::Collateral,
            value_modifier: 100,
            max_staleness: 0,
            token_features: Default::default(),
        },
    );
    send_and_confirm(&ctx.rpc(), &[ix], &[&ctx.airspace_authority]).await?;

    // A user should not be able to register an adapter collateral as a deposit position
    // let result = user_a.create_deposit_position(token_a).await;
    // assert_custom_program_error(glow_margin::ErrorCode::InvalidPositionOwner, result);
    user_a.create_deposit_position(token_b).await?;

    // A user should not be able to register adapter collateral

    return Ok(());
}
