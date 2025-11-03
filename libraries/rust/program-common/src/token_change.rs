use anchor_lang::{AnchorDeserialize, AnchorSerialize};

/// Interface for changing the token value of an account through pool instructions
#[derive(AnchorSerialize, AnchorDeserialize, Debug, Clone, Copy)]
pub struct TokenChange {
    /// The kind of change to be applied
    pub kind: ChangeKind,
    /// The number of tokens applied in the change
    pub tokens: u64,
}

impl TokenChange {
    /// Sets a position's balance to the supplied value, increasing or decreasing
    /// the balance depending on the instruction type.
    ///
    /// Withdrawing with `set(0)` will withdraw all tokens in an account.
    /// Borrowing with `set(100_000)` will borrow additional tokens until the
    /// provided value is reached. If there are already 40_000 tokens borrowed,
    /// an additional 60_000 will be borrowed.
    pub const fn set_source(value: u64) -> Self {
        Self {
            kind: ChangeKind::SetSourceTo,
            tokens: value,
        }
    }

    pub const fn set_destination(value: u64) -> Self {
        Self {
            kind: ChangeKind::SetDestinationTo,
            tokens: value,
        }
    }

    /// Shifts a position's balance by the supplied value, increasing or decreasing
    /// the balance depending on the instruction type.
    ///
    /// Withdrawing with `shift(100_000)` tokens will decrease a balance by the amount.
    /// Depositing with `shift(100_000)` tokens will increase a balance by the amount.
    ///
    /// Refer to the various instructions for the behavior of when instructions can
    /// fail.
    pub const fn shift(value: u64) -> Self {
        Self {
            kind: ChangeKind::ShiftBy,
            tokens: value,
        }
    }

    // /// The amount of the token change, expressed as tokens.
    // ///
    // /// [Amount] can also be notes when interacting with pools, however it is
    // /// always set to tokens for `TokenChange`.
    // pub fn amount(&self) -> Amount {
    //     Amount::tokens(self.tokens)
    // }
}

#[derive(AnchorSerialize, AnchorDeserialize, Debug, Clone, Copy)]
#[repr(u8)]
pub enum ChangeKind {
    SetSourceTo,
    SetDestinationTo,
    ShiftBy,
}
