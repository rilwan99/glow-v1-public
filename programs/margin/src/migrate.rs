use anchor_lang::prelude::*;

use crate::{TokenAdmin, TokenKind};

#[account]
#[derive(Debug, Eq, PartialEq)]
pub struct TokenConfig {
    /// The mint for the token
    pub mint: Pubkey,

    /// The token program of the mint
    pub mint_token_program: Pubkey,

    /// The mint for the underlying token represented, if any
    pub underlying_mint: Pubkey,

    /// The program of the underlying token represented, if any
    pub underlying_mint_token_program: Pubkey,

    /// The space this config is valid within
    pub airspace: Pubkey,

    /// Description of this token
    ///
    /// This determines the way the margin program values a token as a position in a
    /// margin account.
    pub token_kind: TokenKind,

    /// A modifier to adjust the token value, based on the kind of token
    pub value_modifier: u16,

    /// The maximum staleness (seconds) that's acceptable for balances of this token
    pub max_staleness: u64,

    /// The administrator of this token, which has the authority to provide information
    /// about (e.g. prices) and otherwise modify position states for these tokens.
    pub admin: TokenAdmin,
}
