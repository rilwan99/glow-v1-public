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

/// Sanity test for the margin system
///
/// This serves as an example for writing mocked integration tests for the
/// margin system. This particular test will create two users which execute
/// a series of deposit/borrow/repay/withdraw actions onto the margin pools
/// via their margin accounts.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "localnet"), serial_test::serial)]
async fn sanity_test() -> Result<(), anyhow::Error> {
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
    let wallet_b = ctx.create_wallet(10).await?;

    // issue permits for the users
    ctx.issue_permit(wallet_a.pubkey()).await?;
    ctx.issue_permit(wallet_b.pubkey()).await?;

    // Create the user context helpers, which give a simple interface for executing
    // common actions on a margin account
    let user_a = ctx
        .margin_client()
        .user(&wallet_a, 0, glow_client::NetworkKind::Localnet)
        .created(Default::default())
        .await?;
    let user_b = ctx
        .margin_client()
        .user(&wallet_b, 0, glow_client::NetworkKind::Localnet)
        .created(Default::default())
        .await?;

    // Create some tokens for each user to deposit
    let user_a_usdc_account = ctx
        .tokens()
        .create_account_funded(env.usdc, &wallet_a.pubkey(), 1_000_000 * ONE_USDC)
        .await?;
    let user_b_tsol_account = ctx
        .tokens()
        .create_account_funded(env.tsol, &wallet_b.pubkey(), 1_000 * ONE_TSOL)
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

    user_a
        .pool_deposit(
            env.usdc,
            Some(user_a_usdc_account),
            TokenChange::shift(usdc_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    user_b
        .pool_deposit(
            env.tsol,
            Some(user_b_tsol_account),
            TokenChange::shift(tsol_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // Verify user tokens have been deposited
    assert_eq!(0, ctx.tokens().get_balance(&user_a_usdc_account).await?);
    assert_eq!(0, ctx.tokens().get_balance(&user_b_tsol_account).await?);

    user_a.refresh_all_pool_positions().await?;
    user_b.refresh_all_pool_positions().await?;

    // Have each user borrow the other's funds
    let usdc_borrow_amount = 1_000 * ONE_USDC;
    let tsol_borrow_amount = 10 * ONE_TSOL;

    user_a
        .borrow(env.tsol, TokenChange::shift(tsol_borrow_amount))
        .await?;

    user_b
        .borrow(env.usdc, TokenChange::shift(usdc_borrow_amount))
        .await?;

    // User should not be able to borrow more than what's in the pool
    let excess_borrow_result = user_a
        .borrow(env.tsol, TokenChange::shift(5_000 * ONE_TSOL))
        .await;

    assert_custom_program_error(
        glow_margin_pool::ErrorCode::InsufficientLiquidity,
        excess_borrow_result,
    );

    // Users repay their loans from margin account
    user_a
        .margin_repay(env.tsol, TokenChange::shift(tsol_borrow_amount))
        .await?;
    user_b
        .margin_repay(env.usdc, TokenChange::shift(usdc_borrow_amount))
        .await?;

    // Clear any remainig dust
    let user_a_tsol_account = ctx
        .tokens()
        .create_account_funded(env.tsol, &wallet_a.pubkey(), ONE_TSOL / 1_000)
        .await?;
    let user_b_usdc_account = ctx
        .tokens()
        .create_account_funded(env.usdc, &wallet_b.pubkey(), ONE_USDC / 1000)
        .await?;

    user_a
        .repay(
            env.tsol,
            &user_a_tsol_account,
            TokenChange::set_destination(0),
        )
        .await?;
    user_b
        .repay(
            env.usdc,
            &user_b_usdc_account,
            TokenChange::set_destination(0),
        )
        .await?;

    // Verify accounting updated
    let usdc_pool = ctx.margin_client().get_pool(env.usdc).await?;
    let tsol_pool = ctx.margin_client().get_pool(env.tsol).await?;

    assert!(usdc_pool.loan_notes == 0);
    assert!(tsol_pool.loan_notes == 0);

    // Users withdraw their funds
    user_a
        .withdraw(env.usdc, &user_a_usdc_account, TokenChange::set_source(0))
        .await?;
    user_b
        .withdraw(env.tsol, &user_b_tsol_account, TokenChange::set_source(0))
        .await?;

    // Now verify that the users got all their tokens back
    assert!(usdc_deposit_amount <= ctx.tokens().get_balance(&user_a_usdc_account).await?);
    assert!(tsol_deposit_amount <= ctx.tokens().get_balance(&user_b_tsol_account).await?);

    // Check users can create deposit positions and deposit/withdraw
    let user_a_usdc_deposit_account = env.usdc.associated_token_address(user_a.address());
    let user_b_tsol_deposit_account = env.tsol.associated_token_address(user_b.address());
    user_a
        .transfer_deposit(
            env.usdc,
            &wallet_a.pubkey(),
            &user_a_usdc_account,
            &user_a_usdc_deposit_account,
            usdc_deposit_amount,
        )
        .await?;
    user_b
        .transfer_deposit(
            env.tsol,
            &wallet_b.pubkey(),
            &user_b_tsol_account,
            &user_b_tsol_deposit_account,
            tsol_deposit_amount,
        )
        .await?;

    assert!(
        usdc_deposit_amount
            <= ctx
                .tokens()
                .get_balance(&user_a_usdc_deposit_account)
                .await?
    );
    assert!(
        tsol_deposit_amount
            <= ctx
                .tokens()
                .get_balance(&user_b_tsol_deposit_account)
                .await?
    );

    // Withdraw deposits
    user_a
        .transfer_deposit(
            env.usdc,
            user_a.owner(),
            &user_a_usdc_deposit_account,
            &user_a_usdc_account,
            usdc_deposit_amount,
        )
        .await?;
    user_b
        .transfer_deposit(
            env.tsol,
            user_b.owner(),
            &user_b_tsol_deposit_account,
            &user_b_tsol_account,
            tsol_deposit_amount,
        )
        .await?;

    // Now verify that the users got all their tokens back
    assert!(usdc_deposit_amount <= ctx.tokens().get_balance(&user_a_usdc_account).await?);
    assert!(tsol_deposit_amount <= ctx.tokens().get_balance(&user_b_tsol_account).await?);

    // Check if we can update the metadata
    ctx.margin_client()
        .configure_margin_pool(
            env.usdc,
            &MarginPoolConfiguration {
                metadata: Some(TokenMetadataParams {
                    token_kind: glow_metadata::TokenKind::Collateral,
                    collateral_weight: MAX_COLLATERAL_VALUE_MODIFIER,
                    max_leverage: MAX_CLAIM_VALUE_MODIFIER,
                    max_staleness: 30,
                    token_features: Default::default(),
                }),
                ..Default::default()
            },
        )
        .await?;

    user_a.refresh_all_position_metadata(&refresher).await?;
    user_b.refresh_all_position_metadata(&refresher).await?;

    let mut user_a_state = ctx.margin_client().get_account(user_a.address()).await?;
    let mut user_b_state = ctx.margin_client().get_account(user_b.address()).await?;

    assert_eq!(
        MAX_COLLATERAL_VALUE_MODIFIER,
        user_a_state
            .get_position_mut(&usdc_pool.deposit_note_mint)
            .unwrap()
            .value_modifier
    );
    assert_eq!(
        MAX_CLAIM_VALUE_MODIFIER,
        user_b_state
            .get_position_mut(&usdc_pool.loan_note_mint)
            .unwrap()
            .value_modifier
    );

    // Close a specific position
    user_a
        .close_pool_position(env.tsol, TokenKind::Collateral)
        .await?;

    // Close all User A empty accounts
    let mut loan_to_token: HashMap<Pubkey, MintInfo> = HashMap::new();
    loan_to_token.insert(
        MarginPoolIxBuilder::new(ctx.airspace_details.address, env.tsol).loan_note_mint,
        env.tsol,
    );
    loan_to_token.insert(
        MarginPoolIxBuilder::new(ctx.airspace_details.address, env.usdc).loan_note_mint,
        env.usdc,
    );
    user_a.close_empty_positions(&loan_to_token).await?;

    // There should be 0 positions in the margin account, as all should be closed.
    user_a_state = ctx.margin_client().get_account(user_a.address()).await?;
    assert_eq!(user_a_state.positions().count(), 0);

    // Close User A's margin account
    user_a.close_account().await?;

    // User B only had a TSOL deposit, they should not be able to close
    // a non-existent loan position by closing both deposit and loan.
    // let b_close_tsol_result = user_b.close_token_positions(&env.tsol).await;
    // // Error ref: https://github.com/project-serum/anchor/blob/v0.23.0/lang/src/error.rs#L171
    // assert_custom_program_error(
    //     anchor_lang::error::ErrorCode::AccountNotInitialized,
    //     b_close_tsol_result,
    // );

    // NOTE: due to how the simulator works, the deposit will be closed
    // as the state gets mutated regardless of an error.
    // So the user should have 3 positions left, but they have 2

    // It should not be possible to close User B account as it is not empty
    let b_close_acc_result = user_b.close_account().await;
    assert_custom_program_error(glow_margin::ErrorCode::AccountNotEmpty, b_close_acc_result);

    // User B had a USDC loan which created a corresponding deposit.
    // They should be able to close all now empty positions
    user_b.close_empty_positions(&loan_to_token).await?;

    // Close User B's account
    user_b.close_account().await?;

    Ok(())
}
