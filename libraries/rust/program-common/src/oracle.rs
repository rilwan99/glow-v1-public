use anchor_lang::{AnchorDeserialize, AnchorSerialize};

#[derive(Default, Debug, Copy, Eq, PartialEq, Clone, AnchorSerialize, AnchorDeserialize)]
pub enum TokenPriceOracle {
    #[default]
    NoOracle,
    PythPull {
        feed_id: [u8; 32],
    },
    /// A redemption rate from a derivative to the underlying (e.g. sUSD:USD, sSOL:SOL)
    /// To be used in future.
    PythPullRedemption {
        feed_id: [u8; 32],
        quote_feed_id: [u8; 32],
    },
}

impl TokenPriceOracle {
    pub fn pyth_feed_id(&self) -> Option<&[u8; 32]> {
        match self {
            TokenPriceOracle::PythPull { feed_id } => Some(feed_id),
            TokenPriceOracle::PythPullRedemption { feed_id, .. } => Some(feed_id),
            _ => None,
        }
    }

    pub fn is_redemption_rate(&self) -> bool {
        matches!(self, TokenPriceOracle::PythPullRedemption { .. })
    }

    pub fn pyth_redemption_feed_id(&self) -> Option<&[u8; 32]> {
        match self {
            TokenPriceOracle::PythPullRedemption { quote_feed_id, .. } => Some(quote_feed_id),
            _ => None,
        }
    }
}

impl serde::Serialize for TokenPriceOracle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            TokenPriceOracle::NoOracle => serializer.serialize_str("NoOracle"),
            TokenPriceOracle::PythPull { feed_id } => {
                let hex = format!("PythPull:0x{}", hex::encode(feed_id));
                serializer.serialize_str(&hex)
            }
            TokenPriceOracle::PythPullRedemption {
                feed_id,
                quote_feed_id,
            } => {
                let hex = format!(
                    "PythPullRedemption:0x{}:0x{}",
                    hex::encode(feed_id),
                    hex::encode(quote_feed_id)
                );
                serializer.serialize_str(&hex)
            }
        }
    }
}
pub mod pyth_feed_ids {
    use super::get_feed_id_from_hex;

    // See https://www.pyth.network/developers/price-feed-ids#stable
    //
    // NOTE: These aren't meant to be exhaustive, we only use them for testing. Liquidators or other
    // code should not rely on them.
    pub const SOL_USD: &str = "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";
    pub const USDC_USD: &str = "0xeaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a";
    pub const USDT_USD: &str = "0x2b89b9dc8fdf9f34709a5b106b472f0f39bb6ca9ce04b0fd7f2e971688e2e53b";
    pub const BTC_USD: &str = "0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43";
    pub const PYUSD_USD: &str =
        "0xc1da1b73d7f01e7ddd54b3766cf7fcd644395ad14f70aa706ec5384c59e76692";
    pub const JUP_USD: &str = "0x0a0408d619e9380abad35060f9192039ed5042fa6f82301d0e48bb52be830996";
    pub const SSOL_SOL_REDEMPTION: &str =
        "0xadd6499a420f809bbebc0b22fbf68acb8c119023897f6ea801688e0d6e391af4";

    pub fn sol_usd() -> [u8; 32] {
        get_feed_id_from_hex(SOL_USD)
    }

    pub fn usdc_usd() -> [u8; 32] {
        get_feed_id_from_hex(USDC_USD)
    }

    pub fn usdt_usd() -> [u8; 32] {
        get_feed_id_from_hex(USDT_USD)
    }

    pub fn btc_usd() -> [u8; 32] {
        get_feed_id_from_hex(BTC_USD)
    }

    pub fn pyusd_usd() -> [u8; 32] {
        get_feed_id_from_hex(PYUSD_USD)
    }

    pub fn jup_usd() -> [u8; 32] {
        get_feed_id_from_hex(JUP_USD)
    }

    pub fn ssol_sol_rr() -> [u8; 32] {
        get_feed_id_from_hex(SSOL_SOL_REDEMPTION)
    }

    // For test purposes
    pub fn gsol_sol_rr() -> [u8; 32] {
        get_feed_id_from_hex("0xadd6499a420f809bbebc0b22fbf68acb8c119023897f6ea80008000000000000")
    }
}

/// Copied from pyth receiver to avoid importing the whole crate
pub fn get_feed_id_from_hex(input: &str) -> [u8; 32] {
    let mut feed_id = [0; 32];
    match input.len() {
        66 => feed_id.copy_from_slice(&hex::decode(&input[2..]).unwrap()),
        64 => feed_id.copy_from_slice(&hex::decode(input).unwrap()),
        _ => panic!("Invalid feed id length"),
    }
    feed_id
}
