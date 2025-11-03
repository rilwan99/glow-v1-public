//! Margin account valuation and forecasting

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub const MARGIN_ACCOUNT_SETUP_LEVERAGE_FRACTION: f64 = 0.5;

#[derive(Debug, thiserror::Error)]
pub enum ValuationError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

#[derive(Serialize, Deserialize)]
pub struct MarginAccountValuationInput {
    pub positions: HashMap<String, WMarginPosition>,
    pub changes: Vec<WMarginPosition>,
    pub prices: HashMap<String, WOraclePrice>,
}

#[derive(Clone)]
pub struct MarginAccountValuation {
    pub liabilities: f64,
    pub required_collateral: f64,
    pub required_setup_collateral: f64,
    pub weighted_collateral: f64,
    pub effective_collateral: f64,
    pub available_collateral: f64,
    pub available_setup_collateral: f64,
    pub assets: f64,
    pub total_positions: u32,
    pub unvalued_positions: u32,
}

impl MarginAccountValuation {
    // Update the new method to use this error type
    pub fn new(val: MarginAccountValuationInput) -> Result<MarginAccountValuation, ValuationError> {
        let MarginAccountValuationInput {
            positions,
            changes,
            prices,
        } = val;

        Ok(Self::value(positions, changes, &prices))
    }

    pub fn setup_leverage() -> f64 {
        MARGIN_ACCOUNT_SETUP_LEVERAGE_FRACTION
    }

    pub fn health_level(&self) -> f64 {
        if self.required_collateral < 0.0
            || self.weighted_collateral < 0.0
            || self.liabilities < 0.0
        {
            // Invalid input, return infinity
            return f64::INFINITY;
        }
        if self.weighted_collateral > 0.0 {
            let ratio =
                ((self.required_collateral + self.liabilities) / self.weighted_collateral) * 100.0;
            100.0 - ratio
        } else if self.required_collateral + self.liabilities > 0.0 {
            0.0
        } else {
            f64::INFINITY
        }
    }

    fn value(
        positions: HashMap<String, WMarginPosition>,
        changes: Vec<WMarginPosition>,
        prices: &HashMap<String, WOraclePrice>,
    ) -> Self {
        // Update positions with changes
        let mut updated = positions;
        for change in changes {
            let position = updated.get_mut(&change.address);
            if let Some(position) = position {
                position.balance += change.balance;
            } else {
                updated.insert(change.address.clone(), change);
            }
        }
        let mut valuation = MarginAccountValuation {
            assets: 0.0,
            liabilities: 0.0,
            required_collateral: 0.0,
            weighted_collateral: 0.0,
            effective_collateral: 0.0,
            available_collateral: 0.0,
            required_setup_collateral: 0.0,
            available_setup_collateral: 0.0,
            total_positions: updated.len() as u32,
            unvalued_positions: 0,
        };
        for position in updated.values() {
            let price = prices.get(&position.token);
            if let Some(price) = price {
                let value =
                    position.balance as f64 * 10.0_f64.powi(position.exponent) * price.price;
                match &position.position_kind {
                    // 0 = NoValue
                    // 1 = Deposit
                    // 3 = AdapterCollateral
                    1 => {
                        valuation.assets += value;
                        valuation.weighted_collateral += value * position.value_modifier;
                    }
                    3 => {
                        valuation.assets += value;
                        valuation.weighted_collateral += value * position.value_modifier;
                    }
                    // 2 = Claim
                    2 => {
                        valuation.liabilities += value;
                        valuation.required_collateral += value / position.value_modifier;
                        valuation.required_setup_collateral += value
                            / (position.value_modifier * MARGIN_ACCOUNT_SETUP_LEVERAGE_FRACTION);
                    }
                    _ => {}
                }
            } else {
                valuation.unvalued_positions += 1;
            }
        }

        valuation.effective_collateral = valuation.weighted_collateral - valuation.liabilities;
        valuation.available_collateral =
            valuation.weighted_collateral - valuation.liabilities - valuation.required_collateral;
        valuation.available_setup_collateral = valuation.weighted_collateral
            - valuation.liabilities
            - valuation.required_setup_collateral;

        valuation
    }
}

#[derive(Serialize, Deserialize)]

pub struct WOraclePrice {
    pub price: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WMarginPosition {
    pub address: String,
    pub token: String,
    // So we can have negative balances when subtracting
    pub balance: i64,
    pub exponent: i32,
    pub position_kind: u8,
    pub value_modifier: f64,
}

impl WMarginPosition {
    pub fn new(
        address: String,
        token: String,
        balance: i64,
        exponent: i32,
        position_kind: u8,
        value_modifier: f64,
    ) -> Self {
        Self {
            address,
            token,
            balance,
            exponent,
            position_kind,
            value_modifier,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_valuation() {
        let positions = HashMap::from_iter([(
            "USDC-deposit".to_string(),
            WMarginPosition::new(
                "USDC-deposit".to_string(),
                "USDC".to_string(),
                10_000_000,
                -6,
                1,
                1.0,
            ),
        )]);
        let prices = HashMap::from_iter([("USDC".to_string(), WOraclePrice { price: 1.0 })]);
        let valuation = MarginAccountValuation::value(positions.clone(), vec![], &prices);

        assert_eq!(valuation.assets, 10.0);
        assert_eq!(valuation.liabilities, 0.0);
        assert_eq!(valuation.required_collateral, 0.0);
        assert_eq!(valuation.weighted_collateral, 10.0);
        assert_eq!(valuation.effective_collateral, 10.0);
        assert_eq!(valuation.available_collateral, 10.0);
        assert_eq!(valuation.total_positions, 1);
        assert_eq!(valuation.health_level(), 100.0);

        // User borrows 100 USDC and deposits all of it
        let add_loan = WMarginPosition::new(
            "USDC-loan".to_string(),
            "USDC".to_string(),
            100_000_000,
            -6,
            2,
            10.0,
        );
        let add_deposit = WMarginPosition::new(
            "USDC-deposit".to_string(),
            "USDC".to_string(),
            100_000_000,
            -6,
            1,
            1.0,
        );
        let valuation = MarginAccountValuation::value(
            positions.clone(),
            vec![add_deposit.clone(), add_loan.clone()],
            &prices,
        );

        assert_eq!(valuation.assets, 110.0);
        assert_eq!(valuation.liabilities, 100.0);
        assert_eq!(valuation.total_positions, 2);
        // The account should be fully levered
        assert_eq!(valuation.health_level(), 0.0);

        // When the account is repaid, the health level should increase
        let reduce_loan = WMarginPosition::new(
            "USDC-loan".to_string(),
            "USDC".to_string(),
            -20_000_000,
            -6,
            2,
            10.0,
        );
        let reduce_deposit = WMarginPosition::new(
            // give it a different name to test that a new position is added
            "USDC-deposit-3".to_string(),
            "USDC".to_string(),
            -20_000_000,
            -6,
            1,
            1.0,
        );
        let valuation = MarginAccountValuation::value(
            positions,
            vec![add_deposit, add_loan, reduce_deposit, reduce_loan],
            &prices,
        );

        assert_eq!(valuation.assets, 90.0);
        assert_eq!(valuation.liabilities, 80.0);
        assert_eq!(valuation.total_positions, 3);
        assert!(valuation.health_level() > 0.05);
    }
}
