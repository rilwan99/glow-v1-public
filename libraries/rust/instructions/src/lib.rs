use anchor_spl::{
    associated_token::{
        get_associated_token_address_with_program_id,
        spl_associated_token_account::instruction::create_associated_token_account_idempotent,
    },
    token::ID as TOKEN_ID,
    token_2022::ID as TOKEN_2022_ID,
};
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};
use thiserror::Error;

pub mod airspace;
pub mod margin;
pub mod margin_pool;

/// Instruction builder for the protocol test service
pub mod test_service;

/// Get the address of a [metadata] account.
///
/// Metadata addresses are PDAs of various metadata types. Refer to `metadata` for
/// the different account types.
pub fn get_metadata_address(airspace: &Pubkey, address: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[airspace.as_ref(), address.as_ref()], &glow_metadata::ID).0
}

/// Get the address of a pyth [PriceUpdateV2] account.
///
/// If this is called in a test context, the owner of the account will be the
/// test service.
pub fn derive_pyth_price_feed_account(
    feed_id: &[u8; 32],
    shard_id: Option<u16>,
    pyth_program: Pubkey,
) -> Pubkey {
    Pubkey::find_program_address(
        &[
            &shard_id.unwrap_or_default().to_le_bytes(),
            feed_id.as_ref(),
        ],
        &pyth_program,
    )
    .0
}

#[derive(Error, Debug)]
pub enum JetIxError {}

pub type IxResult<T> = Result<T, JetIxError>;

/// A mint and its token program
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash)]
pub struct MintInfo {
    pub address: Pubkey,
    pub is_token_2022: bool,
}

impl MintInfo {
    /// Create the native mint (SOL)
    pub const fn native() -> Self {
        Self {
            address: anchor_spl::token::spl_token::native_mint::ID,
            is_token_2022: false,
        }
    }

    /// Create with a provided token program
    pub fn with_token_program(address: Pubkey, token_program: Pubkey) -> Self {
        Self {
            address,
            is_token_2022: if token_program == TOKEN_2022_ID {
                true
            } else if token_program == TOKEN_ID {
                false
            } else {
                panic!("Unsupported token program {token_program}")
            },
        }
    }

    /// Create with the token 2022 program
    pub const fn with_token_2022(address: Pubkey) -> Self {
        Self {
            address,
            is_token_2022: true,
        }
    }

    /// Create with the legacy token program
    pub const fn with_legacy(address: Pubkey) -> Self {
        Self {
            address,
            is_token_2022: false,
        }
    }

    /// The token program used by this mint
    #[inline]
    pub const fn token_program(self) -> Pubkey {
        if self.is_token_2022 {
            TOKEN_2022_ID
        } else {
            TOKEN_ID
        }
    }

    /// Derive an associate d token account address
    pub fn associated_token_address(&self, authority: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(
            authority,
            &self.address,
            &self.token_program(),
        )
    }

    /// Create an instruction for an associated token account
    pub fn create_associated_token_account_idempotent(
        &self,
        owner: &Pubkey,
        payer: &Pubkey,
    ) -> Instruction {
        create_associated_token_account_idempotent(
            payer,
            owner,
            &self.address,
            &self.token_program(),
        )
    }
}
