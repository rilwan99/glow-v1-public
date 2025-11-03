use glow_client::{margin::MarginAccountClient, ClientResult, GlowClient};
use glow_instructions::MintInfo;
use glow_margin_sdk::solana::transaction::TransactionBuilderExt;
use solana_sdk::{clock::Clock, pubkey::Pubkey};

use crate::{
    context::{token::PriceUpdate, TestContext},
    TestDefault,
};

pub struct Token {
    pub mint: Pubkey,
    pub decimals: u8,
    pub is_token_2022: bool,
}

impl From<&Token> for MintInfo {
    fn from(val: &Token) -> Self {
        MintInfo {
            address: val.mint,
            is_token_2022: val.is_token_2022,
        }
    }
}

impl Token {
    pub fn from_context(ctx: &TestContext, name: &str) -> Self {
        let actual_name = format!("{}-{name}", &ctx.config.airspaces[0].name);

        ctx.config
            .tokens
            .iter()
            .find(|t| t.name == actual_name)
            .map(|t| Self {
                mint: t.mint,
                decimals: t.decimals,
                is_token_2022: t.token_program == anchor_spl::token_2022::ID,
            })
            .unwrap()
    }

    pub fn amount(&self, value: f64) -> u64 {
        amount_from_f64(self, value)
    }
}

pub async fn add_time(ctx: &TestContext, increment: i64) {
    let current = ctx.rpc().get_clock().await.unwrap();

    ctx.rpc()
        .set_clock(Clock {
            unix_timestamp: current.unix_timestamp + increment,
            ..current
        })
        .await
        .unwrap()
}

/// change price of a token
pub async fn set_price(ctx: &TestContext, token: &Token, price: f64, confidence: f64) {
    ctx.inner
        .update_price(
            token.mint,
            &PriceUpdate::test_default()
                .with_price(price)
                .with_confidence(confidence),
        )
        .send_and_confirm(&ctx.rpc())
        .await
        .unwrap();
}

/// airdrop tokens to a user client
pub async fn airdrop(user: &GlowClient, token: &Token, amount: u64) {
    user.test_service()
        .token_request(token.into(), amount)
        .await
        .unwrap();

    user.state().sync_all().await.unwrap();
}

/// sync all user client states
pub async fn sync_all(users: &[GlowClient]) {
    for user in users {
        user.state().sync_all().await.unwrap();
    }
}

pub async fn deposit(
    account: &MarginAccountClient,
    token: &Token,
    amount: u64,
) -> ClientResult<()> {
    account.deposit(token.into(), amount, None).await?;
    account.sync().await.unwrap();

    Ok(())
}

pub async fn withdraw(
    account: &MarginAccountClient,
    token: &Token,
    amount: u64,
) -> ClientResult<()> {
    account.withdraw(token.into(), amount, None).await?;
    account.sync().await.unwrap();

    Ok(())
}

pub async fn pool_lend(
    account: &MarginAccountClient,
    token: &Token,
    amount: u64,
) -> ClientResult<()> {
    account.pool(token.into()).lend(amount).await?;
    account.sync().await.unwrap();

    Ok(())
}

pub async fn pool_borrow(
    account: &MarginAccountClient,
    token: &Token,
    amount: u64,
) -> ClientResult<()> {
    account.pool(token.into()).borrow(amount, None).await?;
    account.sync().await.unwrap();

    Ok(())
}

pub async fn pool_repay(
    account: &MarginAccountClient,
    token: &Token,
    amount: Option<u64>,
) -> ClientResult<()> {
    account.pool(token.into()).repay(amount).await?;
    account.sync().await.unwrap();

    Ok(())
}

pub async fn pool_withdraw(
    account: &MarginAccountClient,
    token: &Token,
    amount: Option<u64>,
) -> ClientResult<()> {
    account.pool(token.into()).withdraw(amount, None).await?;
    account.sync().await.unwrap();

    Ok(())
}

pub fn position_balance(account: &MarginAccountClient, token: &Token) -> u64 {
    account
        .positions()
        .iter()
        .find(|p| p.token == token.mint)
        .unwrap()
        .balance
}

pub fn wallet_balance(user: &GlowClient, token: &Token) -> u64 {
    let mint_info = MintInfo::from(token);
    user.wallet_balance(mint_info)
}

pub fn amount_from_f64(token: &Token, amount: f64) -> u64 {
    let exponent = token.decimals as u32;
    let one = 10i64.pow(exponent) as f64;

    (one * amount).round() as u64
}

pub fn amount_to_f64(token: &Token, amount: u64) -> f64 {
    let exponent = token.decimals as u32;
    let one = 10i64.pow(exponent) as f64;

    amount as f64 / one
}
