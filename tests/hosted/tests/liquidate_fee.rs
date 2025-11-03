use std::u64;

use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anyhow::Error;

use glow_instructions::{margin::derive_liquidation, test_service::swap_slippy_pool, MintInfo};
use glow_margin::{AccountFeatureFlags, LiquidationState, TokenKind};
use glow_margin_pool::{MarginPoolConfig, PoolFlags};
use glow_margin_sdk::{
    ix_builder::MarginPoolIxBuilder,
    solana::transaction::{TransactionBuilderExt, WithSigner},
    tokens::TokenPrice,
    tx_builder::{MarginActionAuthority, TokenDepositsConfig},
};
use glow_program_common::{oracle::TokenPriceOracle, token_change::TokenChange};
use glow_simulation::assert_custom_program_error;
use hosted_tests::{slippy::TestSlippyPool, tokens::preset_token_configs::*};

use solana_sdk::native_token::LAMPORTS_PER_SOL;
use solana_sdk::signature::Signer;

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

#[tokio::test(flavor = "multi_thread")]
async fn test_swap_low_slippage_with_fees() -> Result<(), anyhow::Error> {
    // Setup:
    //    - To create 3 mints (already created in the test, see TestEnv above) ✅
    //    - To create a SlippyPool (call the InitSlippyPool instruction) ✅
    //    - Fund the slippy pool (directly by token transfer) ✅
    //    - (authorize slippy pool?)
    // User A deposits 10'000 USDC
    // User B deposits 50'000 USDT
    // User A borrows 30'000 USDT, and swaps it for SOL using Slippy,
    //    - with an exchange rate of 1 SOL = 100 USDT
    //    - they incur slippage of 0%
    // The price of SOL decreases from 100 USD to 80 USD
    // The liquidator steps in to liquidate the user.
    //    - the liquidator sells the user's SOL for USDT in slippy again.
    //    - it incurs slippage of 2%
    //    - [The correct output after changing the program should be that the liquidator only get 3% of its fee (being 5% fee - slippage loss)]
    // The account must be healthy after the liquidation
    let ctx = margin_test_context!();

    // create position metadata refresher
    let refresher = ctx.generate_key();
    ctx.margin_config_ix()
        .configure_position_config_refresher(refresher.pubkey(), true)
        .with_signer(&ctx.airspace_authority)
        .send_and_confirm(&ctx.rpc())
        .await?;

    let env = setup_environment(&ctx).await?;

    // Create a slippy pool
    let tsol_usdt_slippy =
        TestSlippyPool::setup_pool(&ctx.rpc(), env.tsol, env.usdt, ctx.payer()).await?;

    // Fund the slippy
    ctx.tokens()
        .mint(
            env.tsol,
            &tsol_usdt_slippy.address,
            &tsol_usdt_slippy.vault_a,
            1_000_000_000_000,
        )
        .await?;
    ctx.tokens()
        .mint(
            env.usdt,
            &tsol_usdt_slippy.address,
            &tsol_usdt_slippy.vault_b,
            300_000_000_000,
        )
        .await?;

    ctx.margin_client()
        .register_adapter(&glow_test_service::ID)
        .await?;

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

    // Deposit user funds into their margin accounts
    let usdc_deposit_amount = 10_000 * ONE_USDC;
    let usdt_deposit_amount = 50_000 * ONE_USDT;

    // Create some tokens for each user to deposit
    let user_a_usdc_account = ctx
        .tokens()
        .create_account_funded(env.usdc, &wallet_a.pubkey(), usdc_deposit_amount)
        .await?;
    let user_b_usdt_account = ctx
        .tokens()
        .create_account_funded(env.usdt, &wallet_b.pubkey(), usdt_deposit_amount)
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

    // User A only deposits 10'000 USDC
    // User B deposits 50'000 USDT
    // User A will borrow the USDT from the pool and swap for TSOL
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
            env.usdt,
            Some(user_b_usdt_account),
            TokenChange::shift(usdt_deposit_amount),
            MarginActionAuthority::AccountAuthority,
        )
        .await?;

    // Verify user tokens have been deposited
    assert_eq!(0, ctx.tokens().get_balance(&user_a_usdc_account).await?);
    assert_eq!(0, ctx.tokens().get_balance(&user_b_usdt_account).await?);

    user_a.refresh_all_pool_positions().await?;
    user_b.refresh_all_pool_positions().await?;
    user_a.refresh_all_position_metadata(&refresher).await?;

    // Slippy swap with a 0% slippage
    let swap_ix = swap_slippy_pool(
        env.tsol,
        env.usdt,
        *user_a.address(),
        30_000 * ONE_USDT,
        false,
        100.0,
        0.0,
    );

    user_a
        .margin_swap(env.usdt, env.tsol, 30_000 * ONE_USDT, &swap_ix)
        .await?;

    // At this point the user should have:
    // - 10'000 USDC deposit =  10'000
    // - 30'000 USDT debt    = -30'000
    // -    300 TSOL deposit =  30'000

    let positions = user_a.positions().await?;
    assert_eq!(positions.len(), 3);
    // The equity in the positions should be 10k
    assert_eq!(
        positions
            .iter()
            .map(|p| if p.kind() == TokenKind::Claim {
                -p.value().as_f64()
            } else {
                p.value().as_f64()
            })
            .sum::<f64>(),
        10000.0f64
    );

    user_a.verify_healthy().await?;

    // The price of SOL falls from 100 to 80.
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
            // Set price to 80 USD +- 1
            &env.tsol.address,
            &TokenPrice {
                exponent: -8,
                price: 8_000_000_000,
                confidence: 100_000_000,
                twap: 8_000_000_000,
                feed_id: *env.tsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    user_a.refresh_all_pool_positions().await?;
    user_b.refresh_all_pool_positions().await?;

    // At this point the user should have:
    // - 10'000 USDC deposit =  10'000
    // - 30'000 USDT debt    = -30'000
    // -    300 TSOL deposit =  24'000
    //                         -------
    //   Total               =   4'000

    user_a.verify_unhealthy().await?;

    let liquidator = ctx.create_liquidator(10).await.unwrap();

    let user_a_liquidation = ctx.margin_client().liquidator(
        &liquidator,
        user_a.owner(),
        user_a.seed(),
        glow_client::NetworkKind::Localnet,
    )?;

    user_a_liquidation.liquidate_begin(true).await?;

    // At this point the valuation of the account is:
    // - Equity                   4'000
    // - Liabilities             30'000
    // - Weighted Collateral     32'800 (10'000 + 24'000 * 0.95)
    // - Required Collateral      3'000
    // - Effective Collateral     2'800
    // - Available Collateral   -   200

    let liquidation_state = ctx
        .rpc()
        .get_account(&derive_liquidation(*user_a.address(), liquidator.pubkey()))
        .await?
        .expect("Liquidation account should exist");
    let liquidation_state =
        bytemuck::pod_read_unaligned::<LiquidationState>(&liquidation_state.data[8..]);

    // The max available collateral limit should be 300 (RC 3'000 * 10%)
    assert_eq!(
        liquidation_state
            .state
            .max_available_collateral_limit()
            .as_f64(),
        300.0
    );

    // The liquidator is constrained to increase collateral to 10% of required collateral.
    // The required collateral is 3'000, thus we need to get available collateral to
    // 300, thus 200 + 300 should be the increase.

    // To achieve this, the liquidator has to sell a combination of tokens that result in
    // at most the 1'300 increase.
    //
    // TSOL     : collateral weight  95%
    // USDC     : collateral weight 100%
    // USDT     : collateral weight 100%
    // USDT debt: max leverage       10x
    //
    // Every $100 of debt that's repaid results in a reduction of required collateral of $10.
    // Every $100 of TSOL that's swapped for USDC or USDT results in a $5 improvement in weighted collateral.
    // EC is also increased when a debt is repaid, by the debt amount, so it's not as simple as selling 500 * 10x.
    // If we sell $3'333 of TSOL to repay the USDT debt, the valuation changes as such:

    /*
    Assets & Liabilities
        USDC                    10'000
        TSOL                    20'667
        USDT debt              -26'667

    Collateral
        Weighted
            USDC * 100%         10'000
            TSOL * 95%          19'633
        Required
            USDT / 10x           2'667
        Effective
            29'633 - 26'667      2'967
        Available
            2'967 - 2'667          300
    */

    // Thus, the liquidator shouldn't be able to sell more than $3'333 worth of SOL in this scenario.
    // At $80, that's 41.66 SOL.

    // Try to sell slightly more than calculated (42 SOL)
    let swap_ix = swap_slippy_pool(
        env.tsol,
        env.usdt,
        *user_a.address(),
        42 * ONE_TSOL,
        true,
        80.0,
        0.00,
    );

    let swap_result = user_a_liquidation
        .swap_and_repay(env.tsol, env.usdt, 42 * ONE_TSOL, None, &swap_ix, None)
        .await;

    assert_custom_program_error(glow_margin::ErrorCode::LiquidationLostValue, swap_result);

    // Try to sell the correct amount of SOL and repay all of it, with 2% slippage.
    let swap_amount = 416625 * ONE_TSOL / 10000;
    let swap_ix = swap_slippy_pool(
        env.tsol,
        env.usdt,
        *user_a.address(),
        swap_amount,
        true,
        80.0,
        0.02,
    );

    user_a_liquidation
        .swap_and_repay(
            env.tsol,
            env.usdt,
            swap_amount,
            None,
            &swap_ix,
            Some(150 * ONE_USDT),
        )
        .await?;

    // TODO: DRY
    let liquidation_state = ctx
        .rpc()
        .get_account(&derive_liquidation(*user_a.address(), liquidator.pubkey()))
        .await?
        .expect("Liquidation account should exist");
    let liquidation_state =
        bytemuck::pod_read_unaligned::<LiquidationState>(&liquidation_state.data[8..]);

    let equity_loss = 66_660_000;
    assert_eq!(
        liquidation_state.state.equity_loss().as_u64(-6),
        equity_loss
    );
    let liquidation_fee = liquidation_state.state.accrued_liquidation_fees[0].amount;
    let expected_liquidation_fee = liquidation_fee.saturating_sub(equity_loss);

    // TODO: get the maximum extractable loss and see if liquidator can extract it in any way
    // let margin_account = user_a.tx.get_account_state().await?;
    // let clock = ctx.rpc().get_clock().await?;
    // let valuation = margin_account.valuation(clock.unix_timestamp as _)?;
    // dbg!(&margin_account, &valuation);

    // let extractable_equity = liquidation_state.state.max_equity_loss() - *liquidation_state.state.equity_loss();
    // let extractable_equety = extractable_equity.as_u64(-6);

    // With the liquidator aware of their wiggle room, are they able to exploit this and withdraw funds?
    // This would be limited to the collateral change
    let liquidator_usdc_account = ctx
        .tokens()
        .create_account(env.usdc, &liquidator.pubkey())
        .await?;
    let result = user_a_liquidation
        .withdraw(
            env.usdc,
            &liquidator_usdc_account,
            TokenChange::shift(150 * ONE_USDC),
        )
        .await;

    assert_custom_program_error(
        glow_margin_pool::ErrorCode::InvalidWithdrawalAuthority,
        result,
    );

    // Check that is_collecting_fees is 0 before collecting fees
    let liquidation_state_before = ctx
        .rpc()
        .get_account(&derive_liquidation(*user_a.address(), liquidator.pubkey()))
        .await?
        .expect("Liquidation account should exist");
    let liquidation_state_before =
        bytemuck::pod_read_unaligned::<LiquidationState>(&liquidation_state_before.data[8..]);
    assert_eq!(
        liquidation_state_before.state.is_collecting_fees, 0,
        "is_collecting_fees should be 0 before collecting fees"
    );

    // Collect liquidation fee
    user_a_liquidation.collect_liquidation_fees().await?;

    // Check that is_collecting_fees is 1 after collecting fees
    let liquidation_state_after = ctx
        .rpc()
        .get_account(&derive_liquidation(*user_a.address(), liquidator.pubkey()))
        .await?
        .expect("Liquidation account should exist");
    let liquidation_state_after =
        bytemuck::pod_read_unaligned::<LiquidationState>(&liquidation_state_after.data[8..]);
    assert_eq!(
        liquidation_state_after.state.is_collecting_fees, 1,
        "is_collecting_fees should be 1 after collecting fees"
    );

    // Get the liquidator's USDT token account
    let liquidator_usdt_account = env.usdt.associated_token_address(&liquidator.pubkey());
    let usdt_balance = ctx
        .rpc()
        .get_token_balance(&liquidator_usdt_account)
        .await?;
    let usdc_balance = ctx
        .rpc()
        .get_token_balance(&liquidator_usdc_account)
        .await?;

    assert_eq!(usdt_balance.unwrap(), expected_liquidation_fee);
    assert_eq!(
        usdc_balance.unwrap(),
        0,
        "BAD, liquidator took funds it shouldn't be able to take"
    );

    user_a_liquidation.verify_healthy().await?;

    user_a_liquidation.liquidate_end(None).await?;

    Ok(())
}
