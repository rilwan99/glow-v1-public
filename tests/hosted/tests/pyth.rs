use glow_program_common::oracle::pyth_feed_ids::*;
use pyth_solana_receiver_sdk::price_update::{PriceFeedMessage, PriceUpdateV2};
use solana_test_framework::*;

use solana_sdk::pubkey::Pubkey;

use glow_simulation::runtime::TestRuntimeRpcClient;
use hosted_tests::program_test::get_programs;

#[tokio::test]
async fn add_pyth_price_feed() {
    let programs = get_programs();
    let mut test_ctx = TestRuntimeRpcClient::new(programs).await;

    let oracle = Pubkey::new_unique();
    let oracle2 = Pubkey::new_unique();
    let price_update = PriceUpdateV2 {
        write_authority: Pubkey::default(),
        verification_level: pyth_solana_receiver_sdk::price_update::VerificationLevel::Full,
        posted_slot: 3,
        price_message: PriceFeedMessage {
            feed_id: sol_usd(),
            price: 10_000_000,
            conf: 100,
            exponent: -8,
            publish_time: 100,
            prev_publish_time: 90,
            ema_price: 10_000_010,
            ema_conf: 110,
        },
    };

    //add the pyth oracle to the context
    test_ctx
        .add_pyth_pull_oracle(oracle, glow_margin_pool::ID, price_update.clone())
        .await
        .unwrap();
    test_ctx
        .add_pyth_pull_oracle(oracle2, glow_margin_pool::ID, price_update.clone())
        .await
        .unwrap();

    let mut ctx = test_ctx;

    // Get pyth price account data from chain
    let price_data = ctx.get_pyth_price_account(oracle).await.unwrap();
    assert_eq!(
        price_data.price_message.feed_id,
        price_update.price_message.feed_id
    );
}
