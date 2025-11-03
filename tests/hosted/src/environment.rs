//! Test helpers for the `environment` crate.  
//! Defines sane defaults that can be used by most tests.

use glow_environment::config::TokenDescription;
use glow_margin::TokenFeatures;
use glow_margin_pool::{MarginPoolConfig, PoolFlags};

use crate::{test_default, TestDefault};

/// High level token definition to simplify test setup when you only care about:
/// - token name
/// - whether there is a lending pool
/// - tenors for any fixed term markets, if any
///
/// Easily converted into a TokenDescription for use with TestContext.
///
/// If you have more complex requirements for your test, you may want to
/// manually create the TokenDescriptions with assistance from the
/// `test_default()` function to fill in the fields you don't care about.
#[derive(Clone)]
pub struct TestToken {
    pub name: String,
    pub margin_pool: bool,
}

impl From<TestToken> for TokenDescription {
    fn from(value: TestToken) -> Self {
        TokenDescription {
            symbol: value.name.clone(),
            name: value.name,
            margin_pool: value.margin_pool.then_some(test_default()),
            ..test_default()
        }
    }
}

impl TestToken {
    /// just a token - no pool or fixed term
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            margin_pool: false,
        }
    }

    /// token with a pool - no fixed term
    pub fn with_pool(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            margin_pool: true,
        }
    }

    pub fn description(self) -> TokenDescription {
        self.into()
    }
}

impl TestDefault for MarginPoolConfig {
    fn test_default() -> Self {
        MarginPoolConfig {
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
        }
    }
}

impl TestDefault for TokenDescription {
    fn test_default() -> Self {
        TokenDescription {
            name: String::from("Default"),
            symbol: String::from("Default"),
            token_program: anchor_spl::token_2022::ID,
            decimals: Some(6),
            precision: 6,
            mint: None,
            max_test_amount: None,
            collateral_weight: 100,
            max_leverage: 20_00,
            margin_pool: None,
            token_oracle: glow_environment::config::OraclePriceConfig::NoOracle,
            pyth_feed_id: None,
            pyth_redemption_feed_id: None,
            max_staleness: 30, // Set a sane default
            token_features: 0,
        }
    }
}
