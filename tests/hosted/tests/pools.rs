use glow_margin_pool::ErrorCode;
use glow_margin_sdk::{solana::transaction::InverseSendTransactionBuilder, tokens::TokenPrice};
use glow_program_common::{oracle::pyth_feed_ids::*, token_change::TokenChange};
use glow_simulation::assert_custom_program_error;
use hosted_tests::{
    margin_test_context,
    setup_helper::{borrow_and_dispatch, setup_token, setup_user},
};

use solana_sdk::native_token::LAMPORTS_PER_SOL;

const ONE_USDC: u64 = 1_000_000;
const ONE_TSOL: u64 = LAMPORTS_PER_SOL;

#[tokio::test]
async fn simple_pool_lend_borrow_workflow() -> anyhow::Result<()> {
    let ctx = margin_test_context!();

    // derive mints for default config tokens
    let (usdc, usdc_oracle) = setup_token(
        &ctx,
        6,
        100,
        400,
        1.0,
        false,
        usdc_usd(),
        Default::default(),
    )
    .await?;
    let (tsol, tsol_oracle) = setup_token(
        &ctx,
        9,
        95,
        400,
        100.0,
        false,
        sol_usd(),
        Default::default(),
    )
    .await?;

    let deposit_amount_usdc_a = 1_000_000 * ONE_USDC;
    let deposit_amount_tsol_a = 10 * ONE_TSOL;
    let deposit_amount_usdc_b = 1000 * ONE_USDC;
    let deposit_amount_tsol_b = 1000 * ONE_TSOL;

    // Create two user wallets to get started and deposit funds
    let mut user_a = setup_user(
        &ctx,
        vec![
            (usdc, deposit_amount_usdc_a, 0),
            (tsol, deposit_amount_tsol_a, 0),
        ],
        Default::default(),
    )
    .await?;
    let mut user_b = setup_user(
        &ctx,
        vec![
            (usdc, deposit_amount_usdc_b, 0),
            (tsol, deposit_amount_tsol_b, 0),
        ],
        Default::default(),
    )
    .await?;

    user_a
        .deposit(usdc, usdc_oracle, deposit_amount_usdc_a)
        .await
        .unwrap();
    user_a
        .deposit(tsol, tsol_oracle, deposit_amount_tsol_a)
        .await
        .unwrap();
    user_b
        .deposit(usdc, usdc_oracle, deposit_amount_usdc_b)
        .await
        .unwrap();
    user_b
        .deposit(tsol, tsol_oracle, deposit_amount_tsol_b)
        .await
        .unwrap();

    let tsol_pool = ctx.margin_client().get_pool(tsol).await?;
    let usdc_pool = ctx.margin_client().get_pool(usdc).await?;
    let tsol_sum = deposit_amount_tsol_a + deposit_amount_tsol_b;
    let usdc_sum = deposit_amount_usdc_a + deposit_amount_usdc_b;
    assert_eq!(tsol_sum, tsol_pool.deposit_notes);
    assert_eq!(usdc_sum, usdc_pool.deposit_notes);
    assert_eq!(0, tsol_pool.loan_notes);
    assert_eq!(0, usdc_pool.loan_notes);

    let borrow_amount_usdc_a = deposit_amount_usdc_b;
    let borrow_amount_tsol_b = deposit_amount_tsol_a;

    // The users borrow the other one's funds
    borrow_and_dispatch(&ctx, &user_b, tsol, tsol_oracle, borrow_amount_tsol_b).await;

    let tsol_pool = ctx.margin_client().get_pool(tsol).await?;
    let exp_tsol_deposit_notes = tsol_sum + borrow_amount_tsol_b;
    assert_eq!(exp_tsol_deposit_notes, tsol_pool.deposit_notes);
    assert_eq!(borrow_amount_tsol_b, tsol_pool.loan_notes);

    borrow_and_dispatch(&ctx, &user_a, usdc, usdc_oracle, borrow_amount_usdc_a).await;

    let usdc_pool = ctx.margin_client().get_pool(usdc).await?;
    let exp_usdc_deposit_notes = usdc_sum + borrow_amount_usdc_a;
    assert_eq!(exp_usdc_deposit_notes, usdc_pool.deposit_notes);
    assert_eq!(borrow_amount_usdc_a, usdc_pool.loan_notes);

    let too_much = TokenChange::shift(5000 * ONE_TSOL);
    let result = vec![user_a.user.tx.borrow(tsol, too_much).await.unwrap()]
        .send_and_confirm_condensed(&ctx.rpc())
        .await;

    assert_custom_program_error(ErrorCode::InsufficientLiquidity, result);

    let tsol_tok_a = &user_a.token_account(tsol).await.unwrap();
    let tsol_tok_b = &user_b.token_account(tsol).await.unwrap();
    let usdc_tok_a = &user_a.token_account(usdc).await.unwrap();
    let usdc_tok_b = &user_b.token_account(usdc).await.unwrap();
    let bal_tsol_a = ctx.tokens().get_balance(tsol_tok_a).await?;
    let bal_usdc_a = ctx.tokens().get_balance(usdc_tok_a).await?;
    let bal_tsol_b = ctx.tokens().get_balance(tsol_tok_b).await?;
    let bal_usdc_b = ctx.tokens().get_balance(usdc_tok_b).await?;

    // Check we have the initial account balance again
    assert_eq!(deposit_amount_tsol_a, bal_tsol_a);
    assert_eq!(deposit_amount_usdc_a, bal_usdc_a);
    assert_eq!(deposit_amount_tsol_b, bal_tsol_b);
    assert_eq!(deposit_amount_usdc_b, bal_usdc_b);

    assert_eq!(borrow_amount_usdc_a, usdc_pool.loan_notes);

    // The users repay their loans
    user_a.margin_repay_all(tsol).await.unwrap();
    user_b.margin_repay_all(tsol).await.unwrap();
    user_a.margin_repay_all(usdc).await.unwrap();
    user_b.margin_repay_all(usdc).await.unwrap();

    // Make sure the users have a healthy status
    user_a.user.verify_healthy().await.unwrap();
    user_b.user.verify_healthy().await.unwrap();

    let tsol_pool = ctx.margin_client().get_pool(tsol).await?;
    let usdc_pool = ctx.margin_client().get_pool(usdc).await?;
    assert_eq!(0, tsol_pool.loan_notes);
    assert_eq!(0, usdc_pool.loan_notes);

    // The users get their initial tokens back
    user_a
        .withdraw_to_wallet(usdc, 1_000_000 * ONE_USDC)
        .await
        .unwrap();
    user_a
        .withdraw_to_wallet(tsol, 10 * ONE_TSOL)
        .await
        .unwrap();
    user_b
        .withdraw_to_wallet(usdc, 1000 * ONE_USDC)
        .await
        .unwrap();
    user_b
        .withdraw_to_wallet(tsol, 1000 * ONE_TSOL)
        .await
        .unwrap();

    // Back to the initial state of empty pools
    let tsol_pool = ctx.margin_client().get_pool(tsol).await?;
    let usdc_pool = ctx.margin_client().get_pool(usdc).await?;
    assert_eq!(0, tsol_pool.deposit_notes);
    assert_eq!(0, usdc_pool.deposit_notes);

    Ok(())
}

#[tokio::test]
async fn max_pool_util_ratio_after_borrow() -> anyhow::Result<()> {
    let ctx = margin_test_context!();

    // derive mints for default config tokens
    let (usdc, usdc_oracle) = setup_token(
        &ctx,
        6,
        100,
        400,
        1.0,
        false,
        usdc_usd(),
        Default::default(),
    )
    .await?;
    let (tsol, tsol_oracle) = setup_token(
        &ctx,
        9,
        95,
        400,
        100.0,
        false,
        sol_usd(),
        Default::default(),
    )
    .await?;

    let deposit_amount_usdc_a = 1_000_000 * ONE_USDC;
    let deposit_amount_tsol_a = 10 * ONE_TSOL;
    let deposit_amount_usdc_b = 1000 * ONE_USDC;
    let deposit_amount_tsol_b = 1000 * ONE_TSOL;

    // Create two user wallets to get started and deposit funds
    let user_a = setup_user(
        &ctx,
        vec![
            (usdc, deposit_amount_usdc_a, 0),
            (tsol, deposit_amount_tsol_a, 0),
        ],
        Default::default(),
    )
    .await?;
    let user_b = setup_user(
        &ctx,
        vec![
            (usdc, deposit_amount_usdc_b, 0),
            (tsol, deposit_amount_tsol_b, 0),
        ],
        Default::default(),
    )
    .await?;

    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &usdc.address,
            &TokenPrice {
                exponent: -8,
                price: 100_000_000,
                confidence: 1_000_000,
                twap: 100_000_000,
                feed_id: *usdc_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;
    ctx.tokens()
        .set_price(
            // Set price to 1 USD +- 0.01
            &tsol.address,
            &TokenPrice {
                exponent: -8,
                price: 20_000_000_000,
                confidence: 1_000_000,
                twap: 20_000_000_000,
                feed_id: *tsol_oracle.pyth_feed_id().unwrap(),
            },
        )
        .await?;

    user_a
        .deposit(usdc, usdc_oracle, deposit_amount_usdc_a)
        .await
        .unwrap();
    user_a
        .deposit(tsol, tsol_oracle, deposit_amount_tsol_a)
        .await
        .unwrap();
    user_b
        .deposit(usdc, usdc_oracle, deposit_amount_usdc_b)
        .await
        .unwrap();
    user_b
        .deposit(tsol, tsol_oracle, deposit_amount_tsol_b)
        .await
        .unwrap();

    let tsol_pool = ctx.margin_client().get_pool(tsol).await?;
    let usdc_pool = ctx.margin_client().get_pool(usdc).await?;
    let tsol_sum = deposit_amount_tsol_a + deposit_amount_tsol_b;
    let usdc_sum = deposit_amount_usdc_a + deposit_amount_usdc_b;
    assert_eq!(tsol_sum, tsol_pool.deposit_notes);
    assert_eq!(usdc_sum, usdc_pool.deposit_notes);
    assert_eq!(0, tsol_pool.loan_notes);
    assert_eq!(0, usdc_pool.loan_notes);

    let borrow_amount_usdc_a = deposit_amount_usdc_b;
    let borrow_amount_tsol_b = deposit_amount_tsol_a;

    // The users borrow the other one's funds
    borrow_and_dispatch(&ctx, &user_b, tsol, tsol_oracle, borrow_amount_tsol_b).await;

    let tsol_pool = ctx.margin_client().get_pool(tsol).await?;
    let exp_tsol_deposit_notes = tsol_sum + borrow_amount_tsol_b;
    assert_eq!(exp_tsol_deposit_notes, tsol_pool.deposit_notes);
    assert_eq!(borrow_amount_tsol_b, tsol_pool.loan_notes);

    borrow_and_dispatch(&ctx, &user_a, usdc, usdc_oracle, borrow_amount_usdc_a).await;

    let usdc_pool = ctx.margin_client().get_pool(usdc).await?;
    let exp_usdc_deposit_notes = usdc_sum + borrow_amount_usdc_a;
    assert_eq!(exp_usdc_deposit_notes, usdc_pool.deposit_notes);
    assert_eq!(borrow_amount_usdc_a, usdc_pool.loan_notes);

    // Total deposited:   1010
    // 95% of total:      959.5
    let too_much = TokenChange::shift(960 * ONE_TSOL);
    let actual = vec![user_a.user.tx.borrow(tsol, too_much).await.unwrap()]
        .send_and_confirm_condensed(&ctx.rpc())
        .await;

    assert_custom_program_error(ErrorCode::ExceedsMaxBorrowUtilRatio, actual);

    let max_amount = TokenChange::shift(958 * ONE_TSOL);
    vec![user_a.user.tx.borrow(tsol, max_amount).await.unwrap()]
        .send_and_confirm_condensed(&ctx.rpc())
        .await
        .unwrap();

    Ok(())
}

#[tokio::test]
async fn pool_deposit_and_borrow_limit() -> anyhow::Result<()> {
    let ctx = margin_test_context!();

    let (usdc, usdc_oracle) = setup_token(
        &ctx,
        6,
        100,
        400,
        1.0,
        false,
        usdc_usd(),
        Default::default(),
    )
    .await?;
    let user = setup_user(
        &ctx,
        vec![(usdc, 3_000_000_000 * ONE_USDC, 0)],
        Default::default(),
    )
    .await?;
    let pool = ctx.margin_client().get_pool(usdc).await?;

    // Default values from setup_helper.rs
    assert_eq!(500_000_000 * ONE_USDC, pool.config.borrow_limit);
    assert_eq!(2_000_000_000 * ONE_USDC, pool.config.deposit_limit);

    let result = user
        .deposit(usdc, usdc_oracle, 2_000_000_001 * ONE_USDC)
        .await;
    assert_custom_program_error(ErrorCode::DepositLimitReached, result);

    // Add liquidity
    user.deposit(usdc, usdc_oracle, 1_000_000_000 * ONE_USDC)
        .await
        .unwrap();

    let result = user.borrow(usdc, usdc_oracle, 500_000_001 * ONE_USDC).await;
    assert_custom_program_error(ErrorCode::BorrowLimitReached, result);

    user.borrow(usdc, usdc_oracle, 500_000_000 * ONE_USDC)
        .await
        .unwrap();

    Ok(())
}
