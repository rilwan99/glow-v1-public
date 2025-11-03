use anchor_lang::prelude::*;

use crate::seeds::TEST_SERVICE_AUTHORITY;

/// A test service authority that owns PDAs
#[account]
pub struct TestServiceAuthority {
    pub seed: [u8; 1],
}

impl TestServiceAuthority {
    pub fn signer_seeds(&self) -> [&[u8]; 2] {
        [TEST_SERVICE_AUTHORITY, self.seed.as_ref()]
    }
}
