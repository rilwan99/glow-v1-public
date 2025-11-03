use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Error, Result};

use glow_instructions::airspace::AirspaceDetails;
use glow_instructions::MintInfo;
use glow_margin::{AccountFeatureFlags, TokenAdmin, TokenConfigUpdate, TokenFeatures, TokenKind};
use glow_margin_sdk::ix_builder::MarginConfigIxBuilder;

use glow_margin_sdk::solana::transaction::{
    InverseSendTransactionBuilder, SendTransactionBuilder, TransactionBuilderExt, WithSigner,
};
use glow_margin_sdk::tokens::TokenPrice;
use glow_margin_sdk::tx_builder::{MarginActionAuthority, TokenDepositsConfig};
use glow_margin_sdk::util::asynchronous::MapAsync;
use glow_program_common::oracle::TokenPriceOracle;
use glow_program_common::token_change::TokenChange;
use glow_test_service::TokenCreateParams;
use rand::distributions::Alphanumeric;
use rand::Rng;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature, Signer};

use glow_margin_pool::{MarginPoolConfig, PoolFlags};
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use tokio::try_join;

use crate::margin_test_context;
use crate::pricing::TokenPricer;
use crate::test_user::{TestLiquidator, TestUser};
use crate::tokens::TokenManager;
use crate::{context::MarginTestContext, margin::MarginPoolSetupInfo};

const ONE_USDC: u64 = 1_000_000;

pub const DEFAULT_POOL_CONFIG: MarginPoolConfig = MarginPoolConfig {
    borrow_rate_0: 10,
    borrow_rate_1: 20,
    borrow_rate_2: 30,
    borrow_rate_3: 40,
    utilization_rate_1: 10,
    utilization_rate_2: 20,
    management_fee_rate: 10,
    flags: PoolFlags::ALLOW_LENDING.bits(),
    borrow_limit: 500_000_000 * ONE_USDC,
    deposit_limit: 2_000_000_000 * ONE_USDC,
    reserved: 0,
};

pub struct TestEnvironment {
    pub mints: Vec<MintInfo>,
    pub users: Vec<TestUser>,
}

fn new_random_token_name() -> String {
    let rng = rand::thread_rng();
    rng.sample_iter(&Alphanumeric)
        .take(4)
        .map(char::from)
        .collect()
}

pub async fn create_token_with_pyth(
    token_manager: &TokenManager,
    mint_authority: Pubkey,
    oracle_authority: Pubkey,
    price: f64,
    decimals: u8,
    is_token_2022: bool,
    feed_id: [u8; 32],
) -> Result<(MintInfo, TokenPriceOracle), Error> {
    let token_name = new_random_token_name();
    let (mint_info, token_oracle) = token_manager
        .create_token_v2(
            &TokenCreateParams {
                symbol: token_name.clone(),
                name: token_name.clone(),
                decimals,
                authority: mint_authority,
                oracle_authority,
                max_amount: u64::MAX,
                source_symbol: "".to_string(),
                price_ratio: 1.0,
                price_oracle: TokenPriceOracle::PythPull { feed_id },
            },
            (price * 100_000_000.0) as i64,
            is_token_2022,
        )
        .await?;
    Ok((mint_info, token_oracle))
}

#[allow(clippy::too_many_arguments)]
pub async fn setup_token(
    ctx: &MarginTestContext,
    decimals: u8,
    collateral_weight: u16,
    leverage_max: u16,
    price: f64,
    is_token_2022: bool,
    pyth_feed_id: [u8; 32],
    token_features: TokenFeatures,
) -> Result<(MintInfo, TokenPriceOracle), Error> {
    let token_manager = ctx.tokens();
    let (mint_info, token_oracle) = create_token_with_pyth(
        &token_manager,
        ctx.mint_authority().pubkey(),
        ctx.oracle_authority().pubkey(),
        price,
        decimals,
        is_token_2022,
        pyth_feed_id,
    )
    .await?;

    let setup = MarginPoolSetupInfo {
        mint_info,
        collateral_weight,
        max_leverage: leverage_max,
        token_kind: TokenKind::Collateral,
        config: DEFAULT_POOL_CONFIG,
        oracle: token_oracle,
        max_staleness: 30, // the typical default
        token_features,
    };
    let price = TokenPrice {
        feed_id: pyth_feed_id,
        exponent: -8,
        price: (price * 100_000_000.0) as i64,
        confidence: 1_000_000,
        twap: 100_000_000,
    };
    let deposit_config = TokenDepositsConfig {
        oracle: token_oracle,
        collateral_weight,
        max_staleness: 30, // Common default
        token_features,
    };
    let margin_client = ctx.margin_client();
    try_join!(
        margin_client.create_pool(&setup),
        token_manager.set_price(&mint_info.address, &price),
        margin_client.configure_token_deposits(mint_info, Some(&deposit_config))
    )?;

    Ok((mint_info, token_oracle))
}

pub async fn users<const N: usize>(ctx: &Arc<MarginTestContext>) -> Result<[TestUser; N]> {
    Ok(create_users(ctx, N).await?.try_into().unwrap())
}

pub async fn liquidators<const N: usize>(
    ctx: &Arc<MarginTestContext>,
) -> Result<[TestLiquidator; N]> {
    Ok((0..N)
        .map_async(|_| TestLiquidator::new(ctx))
        .await?
        .try_into()
        .unwrap())
}

pub async fn tokens<const N: usize>(
    ctx: &MarginTestContext,
) -> Result<([(MintInfo, TokenPriceOracle); N], TokenPricer)> {
    let (tokens, pricer) = create_tokens(ctx, N).await?;

    Ok((tokens.try_into().unwrap(), pricer))
}

pub async fn create_users(ctx: &Arc<MarginTestContext>, n: usize) -> Result<Vec<TestUser>> {
    (0..n)
        .map_async(|_| setup_user(ctx, vec![], Default::default()))
        .await
}

pub async fn create_tokens(
    ctx: &MarginTestContext,
    n: usize,
) -> Result<(Vec<(MintInfo, TokenPriceOracle)>, TokenPricer)> {
    let tokens: Vec<(MintInfo, _)> = (0..n)
        .map_async(|_| {
            let mut rng = rand::thread_rng();
            setup_token(
                ctx,
                9,
                1_00,
                4_00,
                1.0,
                false,
                rng.gen(),
                TokenFeatures::default(),
            )
        })
        .await?;
    let owner = ctx.solana.payer().pubkey();
    let token_manager = ctx.tokens();
    let vaults = tokens
        .iter()
        .map_async(|(mint, _)| async {
            token_manager
                .create_account_funded(*mint, &owner, u64::MAX / 4)
                .await
        })
        .await?;
    let vaults = tokens
        .clone()
        .into_iter()
        .map(|(info, _)| info.address)
        .zip(vaults)
        .collect::<HashMap<Pubkey, Pubkey>>();
    let pricer = TokenPricer::new(&ctx.solana, vaults);

    Ok((tokens, pricer))
}

/// (token_mint, balance in wallet, balance in pools)
pub async fn setup_user(
    ctx: &Arc<MarginTestContext>,
    tokens: Vec<(MintInfo, u64, u64)>,
    features: AccountFeatureFlags,
) -> Result<TestUser> {
    // Create our two user wallets, with some SOL funding to get started
    let wallet = ctx.solana.create_wallet(10).await?;

    // Add an airspace permit for the user
    ctx.issue_permit(wallet.pubkey()).await?;

    // Create the user context helpers, which give a simple interface for executing
    // common actions on a margin account
    let user = ctx
        .margin_client()
        .user(&wallet, 0, glow_client::NetworkKind::Localnet)
        .created(features)
        .await?;

    let mut mint_to_token_account = HashMap::new();
    for (mint, in_wallet, in_pool) in tokens {
        // Create some tokens for each user to deposit
        let token_account = ctx
            .tokens()
            .create_account_funded(mint, &wallet.pubkey(), in_wallet + in_pool)
            .await?;
        mint_to_token_account.insert(mint, token_account);

        if in_pool > 0 {
            // Deposit user funds into their margin accounts
            user.pool_deposit(
                mint,
                Some(token_account),
                TokenChange::shift(in_pool),
                MarginActionAuthority::AccountAuthority,
            )
            .await?;
        }

        // Verify user tokens have been deposited
        assert_eq!(in_wallet, ctx.tokens().get_balance(&token_account).await?);
    }

    let test_user = TestUser {
        ctx: ctx.clone(),
        user,
        mint_to_token_account,
    };

    test_user
        .ctx
        .solana
        .rpc()
        .send_and_confirm_condensed(test_user.refresh_positions_with_oracles_txs().await?)
        .await?;

    Ok(test_user)
}

pub async fn register_deposit(
    rpc: &Arc<dyn SolanaRpcClient>,
    airspace: Pubkey,
    airspace_authority: &Keypair,
    mint: MintInfo,
    collateral_weight: Option<u16>,
    feed_id: [u8; 32],
) -> Result<Signature> {
    let config_builder = MarginConfigIxBuilder::new(
        AirspaceDetails {
            name: "".to_string(),
            address: airspace,
            authority: airspace_authority.pubkey(),
        },
        rpc.payer().pubkey(),
    );
    config_builder
        .configure_token(
            mint.address,
            TokenConfigUpdate {
                underlying_mint: mint.address,
                underlying_mint_token_program: mint.token_program(),
                admin: TokenAdmin::Margin {
                    oracle: TokenPriceOracle::PythPull { feed_id },
                },
                token_kind: TokenKind::Collateral,
                value_modifier: collateral_weight.unwrap_or(100),
                max_staleness: 30, // Use the common default
                token_features: TokenFeatures::empty(),
            },
        )
        .with_signer(airspace_authority)
        .send_and_confirm(rpc)
        .await
}

/// Environment where no user has a balance
pub async fn build_environment_with_no_balances(
    test_name: &str,
    number_of_mints: u64,
    number_of_users: u64,
) -> Result<(Arc<MarginTestContext>, TestEnvironment), Error> {
    let ctx = margin_test_context!(test_name);
    let mut rng = rand::thread_rng();
    let mut mints: Vec<MintInfo> = Vec::new();
    for _ in 0..number_of_mints {
        let (mint, _) = setup_token(
            &ctx,
            6,
            1_00,
            10_00,
            1.0,
            false,
            rng.gen(),
            TokenFeatures::default(),
        )
        .await?;
        mints.push(mint);
    }
    let mut users: Vec<TestUser> = Vec::new();
    for _ in 0..number_of_users {
        users.push(setup_user(&ctx, vec![], Default::default()).await?);
    }

    Ok((
        ctx,
        TestEnvironment {
            mints,
            users,
            // liquidator,
        },
    ))
}

/// Environment where every user has 100 of every token in their wallet but no pool deposits
pub async fn build_environment_with_raw_token_balances(
    name: &str,
    number_of_mints: u64,
    number_of_users: u64,
) -> Result<(Arc<MarginTestContext>, TestEnvironment), Error> {
    let ctx = margin_test_context!(name);
    // let liquidator = ctx.create_liquidator(100).await?;
    let mut mints: Vec<MintInfo> = Vec::new();
    let mut wallets: Vec<(MintInfo, u64, u64)> = Vec::new();
    let mut rng = rand::thread_rng();
    for _ in 0..number_of_mints {
        let mint = setup_token(
            &ctx,
            6,
            1_00,
            10_00,
            1.0,
            false,
            rng.gen(),
            TokenFeatures::default(),
        )
        .await?;
        mints.push(mint.0);
        wallets.push((mint.0, 100, 0));
    }
    let mut users: Vec<TestUser> = Vec::new();
    for _ in 0..number_of_users {
        users.push(setup_user(&ctx, wallets.clone(), Default::default()).await?);
    }

    Ok((
        ctx,
        TestEnvironment {
            mints,
            users,
            // liquidator,
        },
    ))
}

pub async fn borrow_and_dispatch(
    ctx: &Arc<MarginTestContext>,
    user: &TestUser,
    mint: MintInfo,
    oracle: TokenPriceOracle,
    value: u64,
) {
    vec![
        ctx.tokens()
            .refresh_to_same_price_tx(&mint.address, oracle)
            .await
            .unwrap(),
        user.user
            .tx
            .borrow(mint, TokenChange::shift(value))
            .await
            .unwrap(),
    ]
    .send_and_confirm_condensed_in_order(&ctx.rpc())
    .await
    .unwrap();
}
