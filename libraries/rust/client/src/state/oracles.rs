use std::collections::HashSet;

use anchor_lang::AccountDeserialize;
use glow_instructions::derive_pyth_price_feed_account;
use glow_program_common::Number128;
use glow_solana_client::rpc::SolanaRpcExtra;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;

use super::AccountStates;
use crate::client::ClientResult;

/// The current state of an oracle that provides pricing information
pub struct PriceOracleState {
    pub price: Number128,
    pub is_valid: bool,
}

/// Sync latest state for all oracles
pub async fn sync(states: &AccountStates) -> ClientResult<()> {
    let mut oracle_address_set = HashSet::new();

    oracle_address_set.extend(states.config.tokens.iter().map(|t| {
        derive_pyth_price_feed_account(
            &t.pyth_feed_id.unwrap(),
            None,
            states.network_kind.pyth_oracle(),
        )
    }));
    oracle_address_set.extend(states.cache.addresses_of::<PriceOracleState>());

    let oracles: Vec<_> = oracle_address_set.drain().collect();

    let accounts = states.network.get_accounts_all(&oracles).await?;
    let current_timestamp_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    for (index, account) in accounts.into_iter().enumerate() {
        let address = oracles[index];

        let account = match account {
            Some(account) => account,
            None => {
                log::error!("oracle {address} does not exist");
                continue;
            }
        };

        let price_account = PriceUpdateV2::try_deserialize(&mut &account.data[..])?;
        let current_price = price_account.price_message;

        let price = Number128::from_decimal(current_price.price, current_price.exponent);
        let state = PriceOracleState {
            price,
            is_valid: current_timestamp_secs.saturating_sub(current_price.publish_time as _) < 60,
        };

        states.cache.set(&address, state);
    }

    Ok(())
}
