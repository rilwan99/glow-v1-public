use std::collections::HashMap;

use glow_instructions::derive_pyth_price_feed_account;
use pyth_solana_receiver_sdk::price_update::{PriceFeedMessage, PriceUpdateV2};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PriceUpdate {
    pub binary: BinaryPriceUpdate,
    pub parsed: Option<Vec<ParsedPriceUpdate>>,
}

/// Data to push onchain.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BinaryPriceUpdate {
    pub data: Vec<String>,
    pub encoding: EncodingType,
}

impl BinaryPriceUpdate {
    /// Decoded price update.
    pub fn decode(&self) -> Result<Vec<Vec<u8>>, OracleFetchError> {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine as _;

        let bytes_vec = match self.encoding {
            EncodingType::Hex => self
                .data
                .iter()
                .map(hex::decode)
                .collect::<Result<_, hex::FromHexError>>()?,
            EncodingType::Base64 => self
                .data
                .iter()
                .map(|d| BASE64.decode(d))
                .collect::<Result<_, base64::DecodeError>>()?,
        };
        Ok(bytes_vec)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, strum::EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum EncodingType {
    Hex,
    Base64,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct ParsedPriceUpdate {
    pub id: String,
    pub price: pyth_sdk::Price,
    pub ema_price: pyth_sdk::Price,
    pub metadata: RpcPriceFeedMetadataV2,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct RpcPriceFeedMetadataV2 {
    pub prev_publish_time: Option<i64>,
    pub proof_available_time: Option<i64>,
    pub slot: Option<i64>,
}

pub fn to_price_update_v2(v: ([u8; 32], ParsedPriceUpdate)) -> PriceUpdateV2 {
    PriceUpdateV2 {
        write_authority: Pubkey::default(),
        verification_level: pyth_solana_receiver_sdk::price_update::VerificationLevel::Full,
        price_message: PriceFeedMessage {
            feed_id: v.0,
            price: v.1.price.price,
            conf: v.1.price.conf,
            exponent: v.1.price.expo,
            publish_time: v.1.price.publish_time,
            prev_publish_time: v.1.price.publish_time - 30,
            ema_price: v.1.ema_price.price,
            ema_conf: v.1.ema_price.conf,
        },
        posted_slot: 0,
    }
}

#[derive(Error, Debug)]
pub enum OracleFetchError {
    #[error("Error decoding hex: {0}")]
    Hex(#[from] hex::FromHexError),

    #[error("Base64 error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("Network request error: {0}")]
    Reqwest(#[from] reqwest::Error),
}

pub async fn get_prices_from_pythnet(
    hermes_url: &str,
    pyth_feed_ids: &[[u8; 32]],
) -> Result<Vec<(Pubkey, PriceUpdateV2)>, OracleFetchError> {
    // Download in chunks of 5 (arbitrarily chosen) to keep URL short
    let ids = pyth_feed_ids
        .iter()
        .map(|id| (hex::encode(id), *id))
        .collect::<HashMap<_, _>>();
    let mut responses = Vec::with_capacity(ids.len());
    for chunk in ids.keys().collect::<Vec<_>>().chunks(5) {
        let param = chunk
            .iter()
            .map(|id| format!("ids%5B%5D=0x{id}"))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!("{}/v2/updates/price/latest?{param}", hermes_url);
        let response = match reqwest::get(url).await {
            Ok(resp) => resp,
            Err(e) => {
                log::error!(
                    "could not get price update for chunk: {chunk:?}, error: {:?}",
                    e,
                );
                continue;
            }
        };
        let price_updates = match response.json::<PriceUpdate>().await {
            Ok(price) => price,
            Err(e) => {
                log::error!(
                    "could not parse price updates for chunk: {chunk:?}, error: {:?}",
                    e,
                );
                continue;
            }
        };
        // Match price updates back to their feed IDs
        for update in price_updates.parsed.unwrap_or_default() {
            let Some(feed_id) = ids.get(&update.id) else {
                continue;
            };
            responses.push((
                derive_pyth_price_feed_account(
                    feed_id,
                    None,
                    pyth_solana_receiver_sdk::PYTH_PUSH_ORACLE_ID,
                ),
                to_price_update_v2((*feed_id, update)),
            ))
        }
    }
    Ok(responses)
}
