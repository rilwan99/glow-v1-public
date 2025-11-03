use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;

use anchor_lang::AnchorDeserialize;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::associated_token::spl_associated_token_account::instruction::create_associated_token_account;
use anchor_spl::{
    associated_token::ID as ASSOCIATED_TOKEN_ID, token::ID as TOKEN_ID,
    token_2022::ID as TOKEN_2022_ID,
};
use anyhow::{Ok, Result};
use futures::stream::FuturesUnordered;
use futures::TryStreamExt;
use glow_instructions::margin::derive_token_config;
use glow_instructions::{derive_pyth_price_feed_account, MintInfo};
use glow_margin::TokenConfig;
use glow_margin_sdk::cat;
use glow_margin_sdk::solana::transaction::{SendTransactionBuilder, TransactionBuilder};
use glow_margin_sdk::tx_builder::MarginActionAuthority;
use glow_margin_sdk::util::asynchronous::{AndAsync, MapAsync};
use glow_program_common::oracle::TokenPriceOracle;
use glow_program_common::token_change::TokenChange;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature, Signer};

use crate::context::MarginTestContext;
use crate::margin::MarginUser;
use crate::tokens::TokenManager;

pub const ONE: u64 = 1_000_000_000;

/// A MarginUser that takes some extra liberties
#[derive(Clone)]
pub struct TestUser {
    pub ctx: Arc<MarginTestContext>,
    pub user: MarginUser,
    pub mint_to_token_account: HashMap<MintInfo, Pubkey>,
}

impl std::fmt::Debug for TestUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestUser")
            .field("user", &self.user.address())
            // .field("liquidator", &self.liquidator.address())
            .field("mint_to_token_account", &self.mint_to_token_account)
            .finish()
    }
}

impl TestUser {
    pub async fn token_account(&mut self, mint: MintInfo) -> Result<Pubkey> {
        let token_account = match self.mint_to_token_account.entry(mint) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => *entry.insert(
                self.ctx
                    .tokens()
                    .create_account(mint, self.user.owner())
                    .await?,
            ),
        };

        Ok(token_account)
    }

    pub async fn ephemeral_token_account(&self, mint: MintInfo, amount: u64) -> Result<Pubkey> {
        let account = self
            .ctx
            .tokens()
            .create_account_funded(mint, self.user.owner(), amount)
            .await?;
        Ok(account)
    }

    pub async fn mint(&mut self, mint: MintInfo, amount: u64) -> Result<()> {
        let token_account = self.token_account(mint).await?;
        self.ctx
            .tokens()
            .mint(mint, self.user.owner(), &token_account, amount)
            .await?;
        Ok(())
    }

    pub async fn deposit(
        &self,
        mint: MintInfo,
        oracle: TokenPriceOracle,
        amount: u64,
    ) -> Result<()> {
        // TODO: this is temporary to help us find where we haven't refreshed oracles
        let oracle_acc = derive_pyth_price_feed_account(
            oracle.pyth_feed_id().unwrap(),
            None,
            glow_test_service::ID,
        );
        let oracle_acc = self.ctx.tokens().get_pyth_price_update(&oracle_acc).await?;

        assert_ne!(oracle_acc.price_message.price, 0);
        assert_ne!(oracle_acc.price_message.ema_price, 0);
        let token_account = self.ephemeral_token_account(mint, amount).await?;
        self.user
            .pool_deposit(
                mint,
                Some(token_account),
                TokenChange::shift(amount),
                MarginActionAuthority::AccountAuthority,
            )
            .await?;

        self.ctx
            .tokens()
            .refresh_to_same_price(&mint.address, oracle)
            .await?;
        Ok(())
    }

    // pub async fn deposit_deprecated(&self, mint: MintInfo, amount: u64) -> Result<()> {
    //     let token_account = self.ephemeral_token_account(mint, amount).await?;
    //     self.user
    //         .pool_deposit_deprecated(mint, &token_account, TokenChange::shift(amount))
    //         .await?;
    //     self.ctx
    //         .tokens()
    //         .refresh_to_same_price(&mint.address, &self.ctx.airspace_details.address)
    //         .await?;
    //     Ok(())
    // }

    pub async fn deposit_from_wallet(&mut self, mint: MintInfo, amount: u64) -> Result<()> {
        let token_account = self.token_account(mint).await?;
        self.user
            .pool_deposit(
                mint,
                Some(token_account),
                TokenChange::shift(amount),
                MarginActionAuthority::AccountAuthority,
            )
            .await
    }

    pub async fn borrow(
        &self,
        mint: MintInfo,
        oracle: TokenPriceOracle,
        amount: u64,
    ) -> Result<Vec<Signature>> {
        let mut txs = vec![
            self.ctx
                .tokens()
                .refresh_to_same_price_tx(&mint.address, oracle)
                .await?,
        ];
        txs.extend(
            self.user
                .tx
                .refresh_all_pool_positions()
                .await?
                .into_iter()
                .map(|v| v.0)
                .collect::<Vec<_>>(),
        );
        txs.push(
            self.user
                .tx
                .borrow(mint, TokenChange::shift(amount))
                .await?,
        );

        self.ctx.rpc().send_and_confirm_condensed(txs).await
    }

    pub async fn borrow_to_wallet(
        &self,
        mint: MintInfo,
        oracle: TokenPriceOracle,
        amount: u64,
    ) -> Result<()> {
        self.borrow(mint, oracle, amount).await?;
        self.withdraw(mint, amount).await
    }

    pub async fn margin_repay(&self, mint: MintInfo, amount: u64) -> Result<()> {
        self.user
            .margin_repay(mint, TokenChange::shift(amount))
            .await
    }

    pub async fn margin_repay_all(&self, mint: MintInfo) -> Result<()> {
        self.user
            .margin_repay(mint, TokenChange::set_destination(0))
            .await
    }

    pub async fn withdraw(&self, mint: MintInfo, amount: u64) -> Result<()> {
        let token_account = self.ephemeral_token_account(mint, 0).await?;
        self.user.refresh_all_pool_positions().await?;
        self.user
            .withdraw(mint, &token_account, TokenChange::shift(amount))
            .await
    }

    pub async fn withdraw_to_wallet(&mut self, mint: MintInfo, amount: u64) -> Result<()> {
        let token_account = self.token_account(mint).await?;
        self.user.refresh_all_pool_positions().await?;
        self.user
            .withdraw(mint, &token_account, TokenChange::shift(amount))
            .await
    }

    pub async fn liquidate_begin(&self, refresh_positions: bool) -> Result<()> {
        let mut txs = if refresh_positions {
            self.refresh_position_oracles_txs().await?
        } else {
            vec![]
        };
        txs.push(self.user.liquidate_begin_tx(refresh_positions).await?);
        self.ctx.rpc().send_and_confirm_condensed(txs).await?;

        Ok(())
    }

    pub async fn verify_healthy(&self) -> Result<()> {
        self.user.verify_healthy().await
    }

    pub async fn verify_unhealthy(&self) -> Result<()> {
        self.user.verify_unhealthy().await
    }

    pub async fn liquidate_end(&self, liquidator: Option<Pubkey>) -> Result<()> {
        self.user.liquidate_end(liquidator).await
    }

    pub async fn refresh_position_oracles_txs(&self) -> Result<Vec<TransactionBuilder>> {
        let tokens = TokenManager::new(self.ctx.solana.clone());
        let positions = self.user.positions().await?;
        let mut builders = Vec::with_capacity(positions.len());
        for position in positions {
            // Find oracle
            let config = derive_token_config(&self.ctx.airspace_details.address, &position.token);
            let account = self
                .ctx
                .rpc()
                .get_account(&config)
                .await?
                .expect("Position has no config");
            let config = TokenConfig::try_from_slice(&account.data).expect("Invalid token config");
            builders.push(
                tokens
                    .refresh_to_same_price_tx(&position.token, config.oracle().unwrap())
                    .await?,
            );
        }
        Ok(builders)
    }

    pub async fn refresh_positions_with_oracles_txs(&self) -> Result<Vec<TransactionBuilder>> {
        let tokens = TokenManager::new(self.ctx.solana.clone());
        Ok(self
            .user
            .tx
            .refresh_all_pool_positions_underlying_to_tx()
            .await?
            .into_iter()
            .map(|(ul, pos)| {
                let tokens = tokens.clone();
                async move {
                    let tx2 = tokens.refresh_to_same_price_tx(&ul, pos.1).await?;
                    Ok((tx2, pos))
                }
            })
            .collect::<FuturesUnordered<_>>()
            .try_collect::<Vec<_>>()
            .await?
            .into_iter()
            .map(|(tx2, tx1)| cat![tx1.0, tx2])
            .collect())
    }
}

pub struct TestLiquidator {
    pub ctx: Arc<MarginTestContext>,
    pub wallet: Keypair,
}

impl std::fmt::Debug for TestLiquidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestLiquidator")
            .field("wallet", &self.wallet)
            .finish()
    }
}

impl TestLiquidator {
    pub async fn new(ctx: &Arc<MarginTestContext>) -> Result<TestLiquidator> {
        Ok(TestLiquidator {
            ctx: ctx.clone(),
            wallet: ctx.create_liquidator(100).await?,
        })
    }

    pub fn for_user(&self, user: &MarginUser) -> Result<TestUser> {
        let liquidation = self.ctx.margin_client().liquidator(
            &self.wallet,
            user.owner(),
            user.seed(),
            glow_client::NetworkKind::Localnet,
        )?;

        Ok(TestUser {
            ctx: self.ctx.clone(),
            user: liquidation,
            mint_to_token_account: HashMap::new(),
        })
    }

    pub async fn begin(&self, user: &MarginUser, refresh_positions: bool) -> Result<TestUser> {
        let test_liquidation = self.for_user(user)?;
        test_liquidation
            .user
            .liquidate_begin(refresh_positions)
            .await?;

        Ok(test_liquidation)
    }

    pub async fn liquidate(
        &self,
        user: &MarginUser,
        _collateral: &Pubkey,
        loan: MintInfo,
        _change: TokenChange,
        repay: u64,
    ) -> Result<()> {
        let liq = self.begin(user, true).await?;
        // Create a fee account for the liquidator
        let liquidator = self.wallet.pubkey();
        let fee_destination = get_associated_token_address_with_program_id(
            &liquidator,
            &loan.address,
            &loan.token_program(),
        );

        if self
            .ctx
            .rpc()
            .get_account(&fee_destination)
            .await?
            .is_none()
        {
            let create_ata_ix = create_associated_token_account(
                &liquidator,
                &liquidator,
                &loan.address,
                &loan.token_program(),
            );
            self.ctx
                .rpc()
                .send_and_confirm_1tx(&[create_ata_ix], [&self.wallet])
                .await?;
        }
        liq.margin_repay(loan, repay).await?;
        liq.liquidate_end(Some(self.wallet.pubkey())).await
    }
}
