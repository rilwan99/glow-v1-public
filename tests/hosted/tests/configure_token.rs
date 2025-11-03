use anyhow::Error;

use glow_instructions::MintInfo;
use glow_margin::{
    TokenAdmin, TokenConfig, TokenConfigUpdate, TokenFeatures, TokenKind, MAX_CLAIM_VALUE_MODIFIER,
    MAX_COLLATERAL_VALUE_MODIFIER, MAX_TOKEN_STALENESS,
};
use glow_margin_sdk::{
    get_state::get_anchor_account,
    solana::transaction::{TransactionBuilderExt, WithSigner},
    tokens::TokenPrice,
};
use glow_program_common::oracle::TokenPriceOracle;
use glow_simulation::assert_custom_program_error;
use glow_test_service::TokenCreateParams;
use solana_sdk::signature::Signer;

use hosted_tests::{
    context::MarginTestContext, margin_test_context, tokens::preset_token_configs::*,
};

struct TestTokens {
    usdc: MintInfo,
    tsol: MintInfo,
    usdc_oracle: TokenPriceOracle,
    tsol_oracle: TokenPriceOracle,
}

async fn setup_test_tokens(ctx: &MarginTestContext) -> Result<TestTokens, Error> {
    let authority = ctx.payer().pubkey();
    let (usdc, usdc_oracle) = ctx
        .tokens()
        .create_token_v2(&usdc_config(authority), 100_000_000, false)
        .await?;
    let (tsol, tsol_oracle) = ctx
        .tokens()
        .create_token_v2(&tsol_config(authority), 20_000_000_000, false)
        .await?;

    // Set initial prices
    ctx.tokens()
        .set_price(
            &usdc.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000, // $1.00
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *usdc_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    ctx.tokens()
        .set_price(
            &tsol.address,
            &TokenPrice {
                exponent: -8,
                price: 10_000_000_000, // $100.00
                confidence: 100_000_000,
                twap: 10_000_000_000,
                feed_id: *tsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    Ok(TestTokens {
        usdc,
        tsol,
        usdc_oracle,
        tsol_oracle,
    })
}

/// Test normal token configuration (happy path)
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_configure_token_happy_path() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("configure_token_happy_path");
    let tokens = setup_test_tokens(&ctx).await?;

    let config_ix = ctx.margin_config_ix();
    let token_config_address = config_ix.derive_token_config(&tokens.usdc.address);

    let account_before = ctx.rpc().get_account(&token_config_address).await?;
    assert!(
        account_before.is_none(),
        "Config account should not exist initially"
    );

    let config_update = TokenConfigUpdate {
        underlying_mint: tokens.usdc.address,
        underlying_mint_token_program: tokens.usdc.token_program(),
        admin: TokenAdmin::Margin {
            oracle: tokens.usdc_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: 95,
        max_staleness: 30,
        token_features: TokenFeatures::USD_STABLECOIN,
    };

    config_ix
        .configure_token(tokens.usdc.address, config_update.clone())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let config: TokenConfig = get_anchor_account(&ctx.rpc(), &token_config_address).await?;
    assert_eq!(config.mint, tokens.usdc.address);
    assert_eq!(config.underlying_mint, tokens.usdc.address);
    assert_eq!(config.airspace, ctx.airspace_details.address);
    assert_eq!(config.admin, config_update.admin);
    assert_eq!(config.token_kind, config_update.token_kind);
    assert_eq!(config.value_modifier, config_update.value_modifier);
    assert_eq!(config.max_staleness, config_update.max_staleness);
    assert_eq!(config.token_features, config_update.token_features);

    Ok(())
}

/// Test updating an existing token configuration
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_configure_token_update() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("configure_token_update");
    let tokens = setup_test_tokens(&ctx).await?;

    let config_ix = ctx.margin_config_ix();
    let token_config_address = config_ix.derive_token_config(&tokens.tsol.address);

    let initial_config = TokenConfigUpdate {
        underlying_mint: tokens.tsol.address,
        underlying_mint_token_program: tokens.tsol.token_program(),
        admin: TokenAdmin::Margin {
            oracle: tokens.tsol_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: 90,
        max_staleness: 20,
        token_features: TokenFeatures::SOL_BASED,
    };

    config_ix
        .configure_token(tokens.tsol.address, initial_config.clone())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let config: TokenConfig = get_anchor_account(&ctx.rpc(), &token_config_address).await?;
    assert_eq!(config.admin, initial_config.admin);
    assert_eq!(config.value_modifier, 90);
    assert_eq!(config.max_staleness, 20);
    assert_eq!(config.token_features, TokenFeatures::SOL_BASED);

    // Update some allowed fields
    let updated_config = TokenConfigUpdate {
        underlying_mint: tokens.tsol.address,
        underlying_mint_token_program: tokens.tsol.token_program(),
        admin: TokenAdmin::Margin {
            oracle: tokens.usdc_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: 85,
        max_staleness: 25,
        token_features: TokenFeatures::SOL_BASED | TokenFeatures::RESTRICTED,
    };

    config_ix
        .configure_token(tokens.tsol.address, updated_config.clone())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let config: TokenConfig = get_anchor_account(&ctx.rpc(), &token_config_address).await?;
    assert_eq!(config.admin, updated_config.admin);
    assert_eq!(config.value_modifier, 85);
    assert_eq!(config.max_staleness, 25);
    assert_eq!(
        config.token_features,
        TokenFeatures::SOL_BASED | TokenFeatures::RESTRICTED
    );

    Ok(())
}

/// Test validation errors and edge cases
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_configure_token_validation_errors() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("configure_token_validation");
    let tokens = setup_test_tokens(&ctx).await?;

    let config_ix = ctx.margin_config_ix();

    // Test 1: Max staleness exceeding limit
    let invalid_staleness_config = TokenConfigUpdate {
        underlying_mint: tokens.usdc.address,
        underlying_mint_token_program: tokens.usdc.token_program(),
        admin: TokenAdmin::Margin {
            oracle: tokens.usdc_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: 100,
        max_staleness: MAX_TOKEN_STALENESS + 1, // Exceeds max
        token_features: TokenFeatures::empty(),
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, invalid_staleness_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::InvalidConfigStaleness, result);

    // Test 2: Collateral value modifier exceeding limit
    let invalid_collateral_modifier_config = TokenConfigUpdate {
        underlying_mint: tokens.usdc.address,
        underlying_mint_token_program: tokens.usdc.token_program(),
        admin: TokenAdmin::Margin {
            oracle: tokens.usdc_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: MAX_COLLATERAL_VALUE_MODIFIER + 1, // Exceeds max
        max_staleness: 30,
        token_features: TokenFeatures::empty(),
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, invalid_collateral_modifier_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(
        glow_margin::ErrorCode::InvalidConfigCollateralValueModifierLimit,
        result,
    );

    // Test 3: Claim value modifier exceeding limit
    let invalid_claim_modifier_config = TokenConfigUpdate {
        underlying_mint: tokens.usdc.address,
        underlying_mint_token_program: tokens.usdc.token_program(),
        admin: TokenAdmin::Adapter(glow_margin_pool::ID),
        token_kind: TokenKind::Claim,
        value_modifier: MAX_CLAIM_VALUE_MODIFIER + 1, // Exceeds max
        max_staleness: 30,
        token_features: TokenFeatures::empty(),
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, invalid_claim_modifier_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(
        glow_margin::ErrorCode::InvalidConfigClaimValueModifierLimit,
        result,
    );

    // Test 4: RESTRICTED flag without other features (should fail)
    let restricted_only_config = TokenConfigUpdate {
        underlying_mint: tokens.usdc.address,
        underlying_mint_token_program: tokens.usdc.token_program(),
        admin: TokenAdmin::Margin {
            oracle: tokens.usdc_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: 100,
        max_staleness: 30,
        token_features: TokenFeatures::RESTRICTED, // Only RESTRICTED, no other features
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, restricted_only_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    // Test 5: Multiple feature flags (should fail - only one at a time allowed)
    let multiple_features_config = TokenConfigUpdate {
        underlying_mint: tokens.usdc.address,
        underlying_mint_token_program: tokens.usdc.token_program(),
        admin: TokenAdmin::Margin {
            oracle: tokens.usdc_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: 100,
        max_staleness: 30,
        token_features: TokenFeatures::USD_STABLECOIN | TokenFeatures::SOL_BASED, // Multiple non-RESTRICTED features
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, multiple_features_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    Ok(())
}

/// Test immutability constraints after initial creation
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_configure_token_immutability() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("configure_token_immutability");
    let tokens = setup_test_tokens(&ctx).await?;

    let config_ix = ctx.margin_config_ix();

    let adapter_program = ctx.generate_key().pubkey();
    let initial_config = TokenConfigUpdate {
        underlying_mint: tokens.usdc.address,
        underlying_mint_token_program: tokens.usdc.token_program(),
        admin: TokenAdmin::Adapter(adapter_program),
        token_kind: TokenKind::AdapterCollateral,
        value_modifier: 95,
        max_staleness: 30,
        token_features: TokenFeatures::USD_STABLECOIN,
    };

    config_ix
        .configure_token(tokens.usdc.address, initial_config.clone())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    // Test 1: Cannot change token kind after creation
    let changed_kind_config = TokenConfigUpdate {
        token_kind: TokenKind::Collateral, // Different from original
        admin: TokenAdmin::Margin {
            oracle: TokenPriceOracle::PythPull { feed_id: [0; 32] },
        },
        ..initial_config.clone()
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, changed_kind_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::InvalidConfigTokenKind, result);

    // Test 2: Cannot change adapter address for Adapter admin
    let changed_adapter_config = TokenConfigUpdate {
        admin: TokenAdmin::Adapter(ctx.generate_key().pubkey()), // Different adapter
        ..initial_config.clone()
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, changed_adapter_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(
        glow_margin::ErrorCode::InvalidConfigAdapterAddressChange,
        result,
    );

    // Test 3: Cannot make margin the owner of AdapterCollateral
    let adapter_to_margin_config = TokenConfigUpdate {
        admin: TokenAdmin::Margin {
            oracle: tokens.usdc_oracle,
        },
        ..initial_config.clone()
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, adapter_to_margin_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::InvalidConfigTokenKind, result);

    // Test 4: Cannot change underlying mint after creation
    let changed_underlying_config = TokenConfigUpdate {
        underlying_mint: tokens.tsol.address,
        ..initial_config.clone()
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, changed_underlying_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(
        glow_margin::ErrorCode::InvalidConfigUnderlyingMintChange,
        result,
    );

    Ok(())
}

/// Test feature flag immutability after creation
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_configure_token_feature_immutability() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("configure_token_feature_immutability");
    let tokens = setup_test_tokens(&ctx).await?;

    let config_ix = ctx.margin_config_ix();

    let initial_config = TokenConfigUpdate {
        underlying_mint: tokens.usdc.address,
        underlying_mint_token_program: tokens.usdc.token_program(),
        admin: TokenAdmin::Margin {
            oracle: tokens.usdc_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: 100,
        max_staleness: 30,
        token_features: TokenFeatures::USD_STABLECOIN | TokenFeatures::RESTRICTED,
    };

    config_ix
        .configure_token(tokens.usdc.address, initial_config.clone())
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    // Test 1: Can toggle RESTRICTED flag
    let toggle_restricted_config = TokenConfigUpdate {
        token_features: TokenFeatures::USD_STABLECOIN, // Remove RESTRICTED
        ..initial_config.clone()
    };

    config_ix
        .configure_token(tokens.usdc.address, toggle_restricted_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    // Test 2: Cannot change non-RESTRICTED feature flags after creation
    let changed_features_config = TokenConfigUpdate {
        token_features: TokenFeatures::SOL_BASED, // Different non-RESTRICTED feature
        ..initial_config.clone()
    };

    let result = config_ix
        .configure_token(tokens.usdc.address, changed_features_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::InvalidFeatureFlags, result);

    Ok(())
}

/// Test boundary values for modifiers
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn test_configure_token_boundary_values() -> Result<(), anyhow::Error> {
    let ctx = margin_test_context!("configure_token_boundaries");
    let tokens = setup_test_tokens(&ctx).await?;

    let config_ix = ctx.margin_config_ix();

    let max_collateral_config = TokenConfigUpdate {
        underlying_mint: tokens.usdc.address,
        underlying_mint_token_program: tokens.usdc.token_program(),
        admin: TokenAdmin::Margin {
            oracle: tokens.usdc_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: MAX_COLLATERAL_VALUE_MODIFIER,
        max_staleness: 30,
        token_features: TokenFeatures::empty(),
    };

    config_ix
        .configure_token(tokens.usdc.address, max_collateral_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let max_claim_config = TokenConfigUpdate {
        underlying_mint: tokens.tsol.address,
        underlying_mint_token_program: tokens.tsol.token_program(),
        admin: TokenAdmin::Adapter(glow_margin_pool::ID),
        token_kind: TokenKind::Claim,
        value_modifier: MAX_CLAIM_VALUE_MODIFIER,
        max_staleness: 30,
        token_features: TokenFeatures::empty(),
    };

    config_ix
        .configure_token(tokens.tsol.address, max_claim_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?; // Should succeed

    let temp_config = TokenCreateParams {
        symbol: "TST".to_string(),
        name: "TEST".to_string(),
        decimals: 6,
        authority: ctx.payer().pubkey(),
        oracle_authority: ctx.payer().pubkey(),
        max_amount: u64::MAX,
        source_symbol: "TEST".to_string(),
        price_ratio: 1.0,
        price_oracle: TokenPriceOracle::PythPull {
            feed_id: *tokens.usdc_oracle.pyth_feed_id().unwrap(),
        },
    };
    let (temp_token, temp_oracle) = ctx
        .tokens()
        .create_token_v2(&temp_config, 100_000_000, false)
        .await?;

    let max_staleness_config = TokenConfigUpdate {
        underlying_mint: temp_token.address,
        underlying_mint_token_program: temp_token.token_program(),
        admin: TokenAdmin::Margin {
            oracle: temp_oracle,
        },
        token_kind: TokenKind::Collateral,
        value_modifier: 100,
        max_staleness: MAX_TOKEN_STALENESS, // Exactly at limit
        token_features: TokenFeatures::empty(),
    };

    config_ix
        .configure_token(temp_token.address, max_staleness_config)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    Ok(())
}
