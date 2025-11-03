use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;

use glow_margin_sdk::solana::keypair::clone;
use glow_margin_sdk::solana::transaction::{SendTransactionBuilder, TransactionBuilder};
use glow_margin_sdk::tokens::TokenPrice;
use glow_margin_sdk::util::asynchronous::{AndAsync, MapAsync};
use glow_program_common::oracle::TokenPriceOracle;
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;

use glow_simulation::Keygen;
use glow_simulation::{runtime::TestRuntimeRpcClient, DeterministicKeygen};

use crate::tokens::TokenManager;

pub const ONE: u64 = 1_000_000_000;

pub struct TokenPricer {
    rpc: Arc<dyn SolanaRpcClient>,
    pub tokens: TokenManager,
    payer: Keypair,
    vaults: HashMap<Pubkey, Pubkey>,
}

impl Clone for TokenPricer {
    fn clone(&self) -> Self {
        Self {
            rpc: self.rpc.clone(),
            tokens: self.tokens.clone(),
            payer: clone(&self.payer),
            vaults: self.vaults.clone(),
        }
    }
}

impl TokenPricer {
    pub fn new_without_swaps(ctx: &TestRuntimeRpcClient) -> Self {
        Self {
            rpc: ctx.rpc().clone(),
            tokens: TokenManager::new(ctx.clone()),
            payer: clone(ctx.payer()),
            vaults: HashMap::new(),
        }
    }

    pub fn new(ctx: &TestRuntimeRpcClient, vaults: HashMap<Pubkey, Pubkey>) -> Self {
        Self {
            rpc: ctx.rpc().clone(),
            tokens: TokenManager::new(ctx.clone()),
            payer: clone(ctx.payer()),
            vaults,
        }
    }

    // pub async fn refresh_all_oracles_timestamps(&self, airspace: &Pubkey) -> Result<()> {
    //     self.refresh_oracles_timestamps(&self.vaults.keys().collect::<Vec<&Pubkey>>(), airspace)
    //         .await
    // }

    // /// Updates oracles to say the same prices with a more recent timestamp
    // pub async fn refresh_oracles_timestamps(
    //     &self,
    //     mints: &[&Pubkey],
    //     airspace: &Pubkey,
    // ) -> Result<()> {
    //     let txs = mints
    //         .iter()
    //         .map_async(|mint| self.tokens.refresh_to_same_price_tx(mint, airspace))
    //         .await?;
    //     self.rpc.send_and_confirm_condensed(txs).await?;

    //     Ok(())
    // }

    // pub async fn summarize_price(&self, mint: &Pubkey, airspace: &Pubkey) -> Result<()> {
    //     println!("price summary for {mint}");
    //     let oracle_price = self.get_oracle_price(mint, airspace).await?;
    //     println!("    oracle: {oracle_price}");

    //     Ok(())
    // }

    // /// Sets price in oracle and swap for only a single asset
    // pub async fn set_price(&self, mint: &Pubkey, airspace: &Pubkey, price: f64) -> Result<()> {
    //     let mut txs = vec![];
    //     let oracle_tx = self.set_oracle_price_tx(mint, airspace, price).await?;
    //     txs.push(oracle_tx);
    //     self.rpc.send_and_confirm_condensed(txs).await?;

    //     Ok(())
    // }

    // /// Efficiently sets prices in oracle and swap for many assets at once
    // pub async fn set_prices(
    //     &self,
    //     mint_prices: Vec<(Pubkey, TokenPriceOracle, f64)>,
    //     airspace: &Pubkey,
    //     refresh_unchanged: bool,
    // ) -> Result<()> {
    //     let mut target_prices: HashMap<Pubkey, TokenPriceOracle, f64> =
    //         mint_prices.clone().into_iter().collect();
    //     let mints = target_prices.clone().into_keys().collect();
    //     let oracle_snapshot = self.oracle_snapshot(&mints, airspace).await?;
    //     target_prices.extend(oracle_snapshot);

    //     let mut txs = vec![];
    //     for (mint, price) in if refresh_unchanged {
    //         target_prices.into_iter().collect()
    //     } else {
    //         mint_prices
    //     } {
    //         txs.push(self.set_oracle_price_tx(&mint, airspace, price).await?)
    //     }

    //     self.rpc.send_and_confirm_condensed(txs).await?;

    //     Ok(())
    // }

    // pub async fn oracle_snapshot(
    //     &self,
    //     blacklist: &HashSet<Pubkey>,
    //     airspace: &Pubkey,
    // ) -> Result<HashMap<Pubkey, f64>> {
    //     Ok(self
    //         .vaults
    //         .keys()
    //         .filter(|m| !blacklist.contains(m))
    //         .map_async(|m| (*m).and_result(self.get_oracle_price(m, airspace)))
    //         .await?
    //         .into_iter()
    //         .collect())
    // }

    // pub async fn set_oracle_price_tx(
    //     &self,
    //     mint: &Pubkey,
    //     feed_id: [u8; 32],
    //     price: f64,
    // ) -> Result<TransactionBuilder> {
    //     let price = (price * 100_000_000.0) as i64;
    //     self.tokens
    //         .set_price_tx(
    //             mint,
    //             &TokenPrice {
    //                 exponent: -8,
    //                 price,
    //                 confidence: 0,
    //                 twap: price as u64,
    //                 feed_id,
    //             },
    //         )
    //         .await
    // }

    // pub async fn get_oracle_price(&self, mint: &Pubkey, airspace: &Pubkey) -> Result<f64> {
    //     let px = self.tokens.get_price(mint, airspace).await?;
    //     let price = px.price_message.price as f64 * (10f64.powf(px.price_message.exponent.into()));

    //     Ok(price)
    // }
}
