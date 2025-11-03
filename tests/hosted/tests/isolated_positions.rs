#![allow(unused)]
use std::collections::HashMap;

use anyhow::Error;

use glow_environment::config::TokenDescription;
use glow_instructions::{margin::derive_token_config, MintInfo};
use glow_margin::{
    AccountFeatureFlags, TokenConfig, TokenFeatures, TokenKind, MAX_CLAIM_VALUE_MODIFIER,
    MAX_COLLATERAL_VALUE_MODIFIER,
};
use glow_margin_pool::{MarginPoolConfig, PoolFlags, TokenMetadataParams};
use glow_margin_sdk::{
    get_state::get_anchor_account,
    ix_builder::{MarginPoolConfiguration, MarginPoolIxBuilder},
    solana::transaction::{TransactionBuilderExt, WithSigner},
    tokens::TokenPrice,
    tx_builder::{MarginActionAuthority, TokenDepositsConfig},
};

use glow_program_common::{
    oracle::{pyth_feed_ids::*, TokenPriceOracle},
    token_change::TokenChange,
};
use glow_simulation::{assert_custom_program_error, send_and_confirm};
use glow_test_service::TokenCreateParams;
use solana_sdk::native_token::LAMPORTS_PER_SOL;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use hosted_tests::{
    context::{MarginTestContext, TestContextSetupInfo},
    margin::MarginPoolSetupInfo,
    margin_test_context,
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
    lstsol: MintInfo,
    tsol: MintInfo,
    rusd: MintInfo,
    lstsol_oracle: TokenPriceOracle,
    tsol_oracle: TokenPriceOracle,
    rusd_oracle: TokenPriceOracle,
}

async fn setup_isolated_environment(ctx: &MarginTestContext) -> Result<TestEnv, Error> {
    let authority = ctx.payer().pubkey();
    let (tsol, tsol_oracle) = ctx
        .tokens()
        .create_token_v2(&tsol_config(authority), 20_000_000_000, false)
        .await?;
    let (lstsol, lstsol_oracle) = ctx
        .tokens()
        .create_token_v2(
            &TokenCreateParams {
                symbol: "LSTSOL".to_string(),
                name: "LST SOL".to_string(),
                decimals: 9,
                authority,
                oracle_authority: authority,
                max_amount: u64::MAX,
                source_symbol: "LSTSOL".to_string(),
                price_ratio: 1.0,
                price_oracle: TokenPriceOracle::PythPull { feed_id: sol_usd() },
            },
            20_000_000_000,
            false,
        )
        .await?;

    let (rusd, rusd_oracle) = ctx
        .tokens()
        .create_token_v2(
            &TokenCreateParams {
                symbol: "RUSD".to_string(),
                name: "Restricted USD".to_string(),
                decimals: 6,
                authority,
                oracle_authority: authority,
                max_amount: u64::MAX,
                source_symbol: "USDC".to_string(),
                price_ratio: 1.0,
                price_oracle: TokenPriceOracle::PythPull {
                    feed_id: usdc_usd(),
                },
            },
            100_000_000,
            true,
        )
        .await?;

    let pools = [
        MarginPoolSetupInfo {
            mint_info: lstsol,
            token_kind: TokenKind::Collateral,
            collateral_weight: 1_00,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: lstsol_oracle,
            max_staleness: 30,
            token_features: TokenFeatures::SOL_BASED,
        },
        MarginPoolSetupInfo {
            mint_info: tsol,
            token_kind: TokenKind::Collateral,
            collateral_weight: 95,
            max_leverage: 4_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: tsol_oracle,
            max_staleness: 30,
            token_features: TokenFeatures::SOL_BASED,
        },
        MarginPoolSetupInfo {
            mint_info: rusd,
            token_kind: TokenKind::Collateral,
            collateral_weight: 95,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: rusd_oracle,
            max_staleness: 30,
            token_features: TokenFeatures::RESTRICTED | TokenFeatures::USD_STABLECOIN,
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
                    token_features: pool_info.token_features,
                }),
            )
            .await?;
        ctx.margin_client().create_pool(&pool_info).await?;
    }

    Ok(TestEnv {
        rusd,
        lstsol,
        tsol,
        rusd_oracle,
        lstsol_oracle,
        tsol_oracle,
    })
}

// Test the various token features
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_token_features() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("test_token_features");

    let authority = ctx.payer().pubkey();

    let symbols = ["AAA", "BBB", "CCC", "DDD", "EEE", "FFF"];

    let mut tokens = Vec::with_capacity(symbols.len());
    for symbol in symbols {
        tokens.push(
            ctx.tokens()
                .create_token_v2(
                    &TokenCreateParams {
                        symbol: symbol.to_string(),
                        name: symbol.to_string(),
                        decimals: 6,
                        authority,
                        oracle_authority: authority,
                        max_amount: u64::MAX,
                        source_symbol: symbol.to_string(),
                        price_ratio: 1.0,
                        price_oracle: TokenPriceOracle::PythPull {
                            feed_id: usdc_usd(),
                        },
                    },
                    100_000_000,
                    true,
                )
                .await?,
        );
    }

    let (token_a, token_a_oracle) = tokens[0];
    let (token_b, token_b_oracle) = tokens[1];
    let (token_c, token_c_oracle) = tokens[2];
    let (token_d, token_d_oracle) = tokens[3];
    let (token_e, token_e_oracle) = tokens[4];
    let (token_f, token_f_oracle) = tokens[5];

    let pool_infos = [
        // All feature flags set
        MarginPoolSetupInfo {
            mint_info: token_a,
            token_kind: TokenKind::Collateral,
            collateral_weight: 95,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: token_a_oracle,
            max_staleness: 30,
            token_features: TokenFeatures::all(),
        },
        // No feature flags set
        MarginPoolSetupInfo {
            mint_info: token_b,
            token_kind: TokenKind::Collateral,
            collateral_weight: 95,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: token_b_oracle,
            max_staleness: 30,
            token_features: TokenFeatures::empty(),
        },
        // Should not create a restricted token if it doesn't have other flags
        MarginPoolSetupInfo {
            mint_info: token_c,
            token_kind: TokenKind::Collateral,
            collateral_weight: 95,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: token_c_oracle,
            max_staleness: 30,
            token_features: TokenFeatures::RESTRICTED,
        },
        // Mixed feature flags
        MarginPoolSetupInfo {
            mint_info: token_d,
            token_kind: TokenKind::Collateral,
            collateral_weight: 95,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: token_d_oracle,
            max_staleness: 30,
            token_features: TokenFeatures::USD_STABLECOIN | TokenFeatures::SOL_BASED,
        },
        // Restricted token with feature
        MarginPoolSetupInfo {
            mint_info: token_e,
            token_kind: TokenKind::Collateral,
            collateral_weight: 95,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: token_e_oracle,
            max_staleness: 30,
            token_features: TokenFeatures::RESTRICTED | TokenFeatures::SOL_BASED,
        },
        MarginPoolSetupInfo {
            mint_info: token_f,
            token_kind: TokenKind::AdapterCollateral,
            collateral_weight: 95,
            max_leverage: 10_00,
            config: DEFAULT_POOL_CONFIG,
            oracle: token_f_oracle,
            max_staleness: 30,
            token_features: TokenFeatures::WBTC_BASED,
        },
    ];

    // Should not be able to register a token that has all feature flags set
    let pool_1 = &pool_infos[0];
    let result = ctx
        .margin_client()
        .configure_token_deposits(
            token_a,
            Some(&TokenDepositsConfig {
                oracle: token_a_oracle,
                collateral_weight: 100,
                max_staleness: 30,
                token_features: pool_1.token_features,
            }),
        )
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);
    let result = ctx.margin_client().create_pool(pool_1).await;
    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    // Should register a token with no feature flags set
    let pool_2 = &pool_infos[1];
    ctx.margin_client()
        .configure_token_deposits(
            token_b,
            Some(&TokenDepositsConfig {
                oracle: token_b_oracle,
                collateral_weight: 100,
                max_staleness: 30,
                token_features: pool_2.token_features,
            }),
        )
        .await?;
    ctx.margin_client().create_pool(pool_2).await?;

    // Should not create a restricted token if it has no features
    let pool_3 = &pool_infos[2];
    let result = ctx
        .margin_client()
        .configure_token_deposits(
            token_c,
            Some(&TokenDepositsConfig {
                oracle: token_c_oracle,
                collateral_weight: 100,
                max_staleness: 30,
                token_features: pool_3.token_features,
            }),
        )
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);
    let result = ctx.margin_client().create_pool(pool_3).await;
    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    let pool_4 = &pool_infos[3];
    let result = ctx
        .margin_client()
        .configure_token_deposits(
            token_d,
            Some(&TokenDepositsConfig {
                oracle: token_d_oracle,
                collateral_weight: 100,
                max_staleness: 30,
                token_features: pool_4.token_features,
            }),
        )
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);
    let result = ctx.margin_client().create_pool(pool_4).await;
    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    let pool_5 = &pool_infos[4];
    ctx.margin_client()
        .configure_token_deposits(
            token_e,
            Some(&TokenDepositsConfig {
                oracle: token_e_oracle,
                collateral_weight: 100,
                max_staleness: 30,
                token_features: pool_5.token_features,
            }),
        )
        .await?;
    ctx.margin_client().create_pool(pool_5).await?;

    let pool_6 = &pool_infos[5];
    ctx.margin_client()
        .configure_token_deposits(
            token_f,
            Some(&TokenDepositsConfig {
                oracle: token_f_oracle,
                collateral_weight: 100,
                max_staleness: 30,
                token_features: pool_6.token_features,
            }),
        )
        .await?;
    ctx.margin_client().create_pool(pool_6).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_isolated_positions() -> Result<(), anyhow::Error> {
    // Create a test runtime with restricted tokens
    let ctx = margin_test_context!("test_isolated_positions");

    // create position metadata refreshere
    let refresher = ctx.generate_key();
    ctx.margin_config_ix()
        .configure_position_config_refresher(refresher.pubkey(), true)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let env = setup_isolated_environment(&ctx).await?;

    // Create our three user wallets, with some SOL funding to get started
    let wallet_a = ctx.create_wallet(10).await?;
    let wallet_b = ctx.create_wallet(10).await?;
    let wallet_c = ctx.create_wallet(10).await?;

    // issue permits for the users
    ctx.issue_permit(wallet_a.pubkey()).await?;
    ctx.issue_permit(wallet_b.pubkey()).await?;
    ctx.issue_permit(wallet_c.pubkey()).await?;

    // Should not be able to create a margin account with the wrong feature flags
    let result = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::all())
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    // Should not be able to create an account that's in violation
    let result = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::VIOLATION)
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    // Create the user context helpers, which give a simple interface for executing
    // common actions on a margin account
    let user_a = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::ACCEPTS_SOL_BASED)
        .await?;
    let user_b = ctx
        .margin_client()
        .user(&wallet_b, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::ACCEPTS_STABLECOINS)
        .await?;
    let user_c = ctx
        .margin_client()
        .user(&wallet_c, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::empty())
        .await?;

    // Create some tokens for each user to deposit
    let user_a_rusd_account = ctx
        .tokens()
        .create_account_funded(env.rusd, &wallet_a.pubkey(), 2_000_000 * ONE_RUSD)
        .await?;
    let user_a_lstsol_account = ctx
        .tokens()
        .create_account_funded(env.lstsol, &wallet_a.pubkey(), 1_000 * ONE_TSOL)
        .await?;
    let user_b_tsol_account = ctx
        .tokens()
        .create_account_funded(env.tsol, &wallet_b.pubkey(), 1_000 * ONE_TSOL)
        .await?;
    let user_b_rusd_account = ctx
        .tokens()
        .create_account_funded(env.rusd, &wallet_b.pubkey(), 2_000_000 * ONE_RUSD)
        .await?;
    let user_c_tsol_account = ctx
        .tokens()
        .create_account_funded(env.tsol, &wallet_c.pubkey(), 1_000 * ONE_TSOL)
        .await?;
    let user_c_rusd_account = ctx
        .tokens()
        .create_account_funded(env.rusd, &wallet_c.pubkey(), 1_000_000 * ONE_RUSD)
        .await?;

    // Set the prices for each token
    ctx.tokens()
        .set_price(
            // Set price to 100 USD +- 1
            &env.lstsol.address,
            &TokenPrice {
                exponent: -8,
                price: 10_000_000_000,
                confidence: 100_000_000,
                twap: 10_000_000_000,
                feed_id: *env.lstsol_oracle.pyth_feed_id().unwrap(),
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
    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &env.rusd.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000,
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *env.rusd_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    // Deposit user funds into their margin accounts
    let lstsol_deposit_amount = 1_000 * ONE_TSOL;
    let tsol_deposit_amount = 1_000 * ONE_TSOL;
    let rusd_deposit_amount = 1_000_000 * ONE_RUSD;

    // User A's account accepts SOL-based tokens, they should be able to deposit
    user_a
        .pool_deposit(
            env.lstsol,
            Some(user_a_lstsol_account),
            TokenChange::shift(lstsol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // The user should not be able to deposit SOL restricted tokens to a stablecoin pool
    let result = user_b
        .pool_deposit(
            env.tsol,
            Some(user_b_tsol_account),
            TokenChange::shift(tsol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::RestrictedToken, result);

    // An account with no restrictions should be able to accept any token
    user_c
        .pool_deposit(
            env.tsol,
            Some(user_c_tsol_account),
            TokenChange::shift(tsol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // Only user B should be able to deposit RUSD as it's a restricted token
    let result = user_a
        .pool_deposit(
            env.rusd,
            Some(user_a_rusd_account),
            TokenChange::shift(rusd_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::RestrictedToken, result);

    let result = user_c
        .pool_deposit(
            env.rusd,
            Some(user_c_rusd_account),
            TokenChange::shift(rusd_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::RestrictedToken, result);

    user_b
        .pool_deposit(
            env.rusd,
            Some(user_b_rusd_account),
            TokenChange::shift(rusd_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // Verify user tokens have been deposited
    assert_eq!(0, ctx.tokens().get_balance(&user_a_lstsol_account).await?);
    assert_eq!(0, ctx.tokens().get_balance(&user_c_tsol_account).await?);
    assert_eq!(
        1_000_000 * ONE_RUSD,
        ctx.tokens().get_balance(&user_b_rusd_account).await?
    );

    user_a.refresh_all_pool_positions().await?;
    user_b.refresh_all_pool_positions().await?;
    user_c.refresh_all_pool_positions().await?;

    // Have each user borrow the other's funds
    let lstsol_borrow_amount = 10 * ONE_TSOL;
    let tsol_borrow_amount = 10 * ONE_TSOL;
    let rusd_borrow_amount = 1_000 * ONE_RUSD;

    user_a
        .borrow(env.tsol, TokenChange::shift(tsol_borrow_amount))
        .await?;

    user_b
        .borrow(env.rusd, TokenChange::shift(rusd_borrow_amount))
        .await?;

    user_c
        .borrow(env.lstsol, TokenChange::shift(lstsol_borrow_amount))
        .await?;

    // User B can't borrow non-stablecoins
    let result = user_b
        .borrow(env.lstsol, TokenChange::shift(lstsol_borrow_amount))
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::RestrictedToken, result);

    // Users A and C can't borrow RUSD as it's restricted
    let result = user_a
        .borrow(env.rusd, TokenChange::shift(rusd_borrow_amount))
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::RestrictedToken, result);

    let result = user_c
        .borrow(env.rusd, TokenChange::shift(rusd_borrow_amount))
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::RestrictedToken, result);

    // A non-margin account user should not be able to deposit into a restricted pool.
    let rusd_pool = MarginPoolIxBuilder::new(ctx.airspace_details.address, env.rusd);
    let wallet_a_rusd_deposit_ata = rusd_pool
        .pool_deposit_mint_info()
        .associated_token_address(&wallet_a.pubkey());
    let create_ata_ix = rusd_pool
        .pool_deposit_mint_info()
        .create_associated_token_account_idempotent(&wallet_a.pubkey(), &wallet_a.pubkey());
    let deposit_ix = rusd_pool.deposit(
        wallet_a.pubkey(),
        None,
        user_a_rusd_account,
        wallet_a_rusd_deposit_ata,
        TokenChange::shift(100),
    );
    let result = send_and_confirm(
        &ctx.rpc(),
        &[create_ata_ix.clone(), deposit_ix],
        &[&wallet_a],
    )
    .await;
    assert_custom_program_error(glow_margin_pool::ErrorCode::PoolPermissionDenied, result);

    // Merely passing a margin account should not bypass this restriction
    let deposit_ix = rusd_pool.deposit(
        wallet_a.pubkey(),
        Some(*user_a.address()),
        user_a_rusd_account,
        wallet_a_rusd_deposit_ata,
        TokenChange::shift(100),
    );
    let result = send_and_confirm(&ctx.rpc(), &[create_ata_ix, deposit_ix], &[&wallet_a]).await;
    assert_custom_program_error(glow_margin_pool::ErrorCode::PoolPermissionDenied, result);

    // Wallet B should be able to deposit into its margin account directly
    let user_b_rusd_deposit_ata = rusd_pool
        .pool_deposit_mint_info()
        .associated_token_address(user_b.address());
    let create_ata_ix = rusd_pool
        .pool_deposit_mint_info()
        .create_associated_token_account_idempotent(user_b.address(), &wallet_b.pubkey());
    let deposit_ix = rusd_pool.deposit(
        wallet_b.pubkey(),
        Some(*user_b.address()),
        user_b_rusd_account,
        user_b_rusd_deposit_ata,
        TokenChange::shift(100),
    );
    send_and_confirm(&ctx.rpc(), &[create_ata_ix, deposit_ix], &[&wallet_b]).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_token_feature_violation() -> anyhow::Result<()> {
    // This tests that token feature violation works correctly.
    // * An unrestricted margin account with a token that becomes restricted should be marked as a violation.
    // * To remedy that violation, the account has to close the violating position.
    let ctx = margin_test_context!("test_token_feature_violation");

    // create position metadata refreshere
    let refresher = ctx.generate_key();
    ctx.margin_config_ix()
        .configure_position_config_refresher(refresher.pubkey(), true)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let env = setup_isolated_environment(&ctx).await?;

    let config = get_anchor_account::<TokenConfig>(
        &ctx.rpc(),
        &derive_token_config(&ctx.airspace_details.address, &env.lstsol.address),
    )
    .await?;
    assert_eq!(config.token_features, TokenFeatures::SOL_BASED);

    // Create our three user wallets, with some SOL funding to get started
    let wallet_a = ctx.create_wallet(10).await?;

    // issue permits for the users
    ctx.issue_permit(wallet_a.pubkey()).await?;

    // Create the user context helpers, which give a simple interface for executing
    // common actions on a margin account
    let user_a = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(AccountFeatureFlags::empty())
        .await?;

    // Create some tokens for each user to deposit
    let user_a_lstsol_account = ctx
        .tokens()
        .create_account_funded(env.lstsol, &wallet_a.pubkey(), 1_000 * ONE_TSOL)
        .await?;

    // Set the prices for each token
    ctx.tokens()
        .set_price(
            // Set price to 100 USD +- 1
            &env.lstsol.address,
            &TokenPrice {
                exponent: -8,
                price: 10_000_000_000,
                confidence: 100_000_000,
                twap: 10_000_000_000,
                feed_id: *env.lstsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    // Deposit user funds into their margin accounts
    let lstsol_deposit_amount = 1_000 * ONE_TSOL;

    // Deposit into an unrestricted posiiton
    user_a
        .pool_deposit(
            env.lstsol,
            Some(user_a_lstsol_account),
            TokenChange::shift(lstsol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // Should not be possible to change the token feature outside of restriction
    let result = ctx
        .margin_client()
        .configure_token_deposits(
            env.lstsol,
            Some(&TokenDepositsConfig {
                oracle: env.lstsol_oracle,
                collateral_weight: 10,
                max_staleness: 0,
                token_features: TokenFeatures::RESTRICTED | TokenFeatures::USD_STABLECOIN,
            }),
        )
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    // Change the token restriction
    ctx.margin_client()
        .configure_margin_pool(
            env.lstsol,
            &MarginPoolConfiguration {
                parameters: None,
                metadata: Some(TokenMetadataParams {
                    token_kind: glow_metadata::TokenKind::Collateral,
                    collateral_weight: 10,
                    max_leverage: 2000,
                    max_staleness: 0,
                    token_features: (TokenFeatures::RESTRICTED | TokenFeatures::SOL_BASED).bits(),
                }),
                token_oracle: None,
            },
        )
        .await?;

    // Check that the token is now restricted
    let config = get_anchor_account::<TokenConfig>(
        &ctx.rpc(),
        &derive_token_config(
            &ctx.airspace_details.address,
            &MarginPoolIxBuilder::new(ctx.airspace_details.address, env.lstsol).deposit_note_mint,
        ),
    )
    .await?;
    assert_eq!(
        config.token_features,
        TokenFeatures::RESTRICTED | TokenFeatures::SOL_BASED
    );

    let account_before = user_a.tx.get_account_state().await?;

    // Update position config
    user_a.refresh_all_position_metadata(&refresher).await?;

    let account_after = user_a.tx.get_account_state().await?;

    assert!(account_before.features.is_empty());
    let position = account_before.positions().next().unwrap();
    assert!(position.token_features.is_empty());
    assert!(account_after
        .features
        .contains(AccountFeatureFlags::VIOLATION));
    let position = account_after.positions().next().unwrap();
    assert!(position
        .token_features
        .contains(TokenFeatures::RESTRICTED | TokenFeatures::SOL_BASED));
    assert_eq!(position.value_modifier, 10);

    // User A should be unable to withdraw without closing the position
    let result = user_a
        .withdraw(env.lstsol, &user_a_lstsol_account, TokenChange::shift(10))
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::TokenFeatureViolation, result);

    // User A should be able to withdraw all tokens to remove the account violation
    user_a
        .withdraw(
            env.lstsol,
            &user_a_lstsol_account,
            TokenChange::set_source(0),
        )
        .await?;

    // After withdrawing, it should not be possible to deposit again into the violating position
    let result = user_a
        .pool_deposit(
            env.lstsol,
            Some(user_a_lstsol_account),
            TokenChange::shift(lstsol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await;
    // The error is PoolPermissionDenied because depositing restricted tokens into an unrestricted position is not allowed.
    assert_custom_program_error(glow_margin_pool::ErrorCode::PoolPermissionDenied, result);

    // Close the position
    let mut loan_to_token: HashMap<Pubkey, MintInfo> = HashMap::new();
    loan_to_token.insert(
        MarginPoolIxBuilder::new(ctx.airspace_details.address, env.lstsol).loan_note_mint,
        env.lstsol,
    );
    user_a.close_empty_positions(&loan_to_token).await?;

    // It should be possible to remove the token restriction
    ctx.margin_client()
        .configure_margin_pool(
            env.lstsol,
            &MarginPoolConfiguration {
                parameters: None,
                metadata: Some(TokenMetadataParams {
                    token_kind: glow_metadata::TokenKind::Collateral,
                    collateral_weight: 10,
                    max_leverage: 2000,
                    max_staleness: 0,
                    token_features: TokenFeatures::SOL_BASED.bits(),
                }),
                token_oracle: None,
            },
        )
        .await?;

    let config = get_anchor_account::<TokenConfig>(
        &ctx.rpc(),
        &derive_token_config(
            &ctx.airspace_details.address,
            &MarginPoolIxBuilder::new(ctx.airspace_details.address, env.lstsol).deposit_note_mint,
        ),
    )
    .await?;
    assert_eq!(config.token_features, TokenFeatures::SOL_BASED);

    // But not all token features
    let result = ctx
        .margin_client()
        .configure_margin_pool(
            env.lstsol,
            &MarginPoolConfiguration {
                parameters: None,
                metadata: Some(TokenMetadataParams {
                    token_kind: glow_metadata::TokenKind::Collateral,
                    collateral_weight: 10,
                    max_leverage: 2000,
                    max_staleness: 0,
                    token_features: TokenFeatures::empty().bits(),
                }),
                token_oracle: None,
            },
        )
        .await;
    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    Ok(())
}
