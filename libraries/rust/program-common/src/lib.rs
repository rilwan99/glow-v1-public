#![allow(clippy::result_large_err)]

mod fp32;
mod functions;
pub mod log;
mod number;
mod number_128;

pub mod interest_pricing;
pub mod oracle;
pub mod pod;
pub mod serialization;
pub mod token_change;
pub mod traits;
pub mod valuation;

#[doc(inline)]
pub use functions::*;

#[doc(inline)]
pub use number::*;

#[doc(inline)]
pub use number_128::*;

#[doc(inline)]
pub use fp32::*;

use solana_program::{pubkey, pubkey::Pubkey};

#[cfg(not(feature = "devnet"))]
mod governor_addresses {
    use super::*;

    pub const PROTOCOL_GOVERNOR_ID: Pubkey = GOVERNOR_MAINNET;
}

#[cfg(feature = "devnet")]
mod governor_addresses {
    use super::*;

    pub const PROTOCOL_GOVERNOR_ID: Pubkey = GOVERNOR_DEVNET;
}

pub use governor_addresses::*;

// Glow Mainnet Multisig
pub const MULTISIG_MAINNET: Pubkey = pubkey!("9TDNaAwrB5ftTbr2JaRLYye7E9jjMKvDLKHWfYkBeKzy");
pub const GOVERNOR_MAINNET: Pubkey = pubkey!("87GuoGyR11ES2zCMskSdXyVPh8bC32CW35sFU8Z7mJm2");

pub const MULTISIG_DEVNET: Pubkey = pubkey!("FHaMZWzFbNKJ75gqZ3XCnnSqZAGZiwHSLoNka4QGYwZ7");
pub const GOVERNOR_DEVNET: Pubkey = pubkey!("8LaqrFSxGwWy5rbC2p2wqF4VkH34qMrLcV4VjaKMQEL6");

pub const JUPITER_V6: Pubkey = pubkey!("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4");

/// The lookup table registry program ID is added here as a convenience to avoid
/// importing the crate just to get the ID.
pub const ADDRESS_LOOKUP_REGISTRY_ID: Pubkey =
    pubkey!("LooKUpVskBihZovMhwhEqCER8jwLFHhF4QMZA5axZnJ");

/// Programs whose return data is safe to try parse.
///
/// The margin program relies on return data to update account states after events,
/// e.g. update balances after refreshing oracles.
/// Other programs may set return data (e.g. as part of CPI events), thus if we blindly check
/// return data for those programs, otherwise correct transactions might fail from parse errors.
///
/// SECURITY: What is considered to be a safe program should be evaluated carefully because
/// return data can come from any program that such safe program calls, if the program doesn't
/// set its own data before ending the CPI call.
#[cfg(feature = "testing")]
pub const SAFE_RETURN_DATA_PROGRAMS: [Pubkey; 2] = [
    pubkey!("CWPeEXnSpELj7tSz9W4oQAGGRbavBtdnhY2bWMyPoo1"), // glow margin pool
    pubkey!("test7JXXboKpc8hGTadvoXcFWN4xgnHLGANU92JKrwA"), // test service as it has a test swap pool
];
#[cfg(not(feature = "testing"))]
pub const SAFE_RETURN_DATA_PROGRAMS: [Pubkey; 1] = [
    pubkey!("CWPeEXnSpELj7tSz9W4oQAGGRbavBtdnhY2bWMyPoo1"), // glow margin pool
];

/// Known external programs whose side effects and event data we want to observe.
/// E.g. Jupiter swaps result in token balance changes that we require to calculate liquidation fees.
pub const KNOWN_EXTERNAL_PROGRAMS: [Pubkey; 1] = [JUPITER_V6]; // Jupiter v6
