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

use glow_program_common::oracle::{pyth_feed_ids::*, TokenPriceOracle};
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
    usdc: MintInfo,
    tsol: MintInfo,
    usdc_oracle: TokenPriceOracle,
    tsol_oracle: TokenPriceOracle,
}

async fn setup_environment(ctx: &MarginTestContext) -> Result<TestEnv, Error> {
    let (usdc, usdc_oracle) = ctx
        .tokens()
        .create_token_v2(&usdc_config(ctx.payer().pubkey()), 100_000_000, false)
        .await?;
    let (tsol, tsol_oracle) = ctx
        .tokens()
        .create_token_v2(&tsol_config(ctx.payer().pubkey()), 20_000_000_000, false)
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
        usdc_oracle,
        tsol_oracle,
    })
}

/// Fee withdrawal sanity test
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn withdraw_fees_test() -> Result<(), anyhow::Error> {
    // Get the mocked runtime
    let ctx = margin_test_context!();

    let env = setup_environment(&ctx).await?;

    let pool_ix_builder = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.usdc);
    let fee_owner = ctx.airspace_authority.pubkey();
    let pool_fee_destination = ctx
        .tokens()
        .create_account(pool_ix_builder.pool_deposit_mint_info(), &fee_owner)
        .await?;
    let usdc_fee_destination = ctx.tokens().create_account(env.usdc, &fee_owner).await?;
    let ix = pool_ix_builder.withdraw_fees(fee_owner, usdc_fee_destination);

    let tx = ctx
        .rpc()
        .create_transaction(&[&ctx.airspace_authority], &[ix])
        .await?;
    ctx.rpc().send_transaction(tx).await?;

    Ok(())
}
