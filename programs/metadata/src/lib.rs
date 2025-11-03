// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Copyright (C) 2024 A1 XYZ, INC.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

// Allow this until fixed upstream
#![allow(clippy::result_large_err)]

use anchor_lang::prelude::*;
use glow_airspace::state::Airspace;
use glow_program_common::oracle::TokenPriceOracle;

declare_id!("yT2ut38wC6A6zsGo2aUgy9kkh8EBuNXYvtmo7aUg1oW");

/// The current version of the [PositionTokenMetadata] account.
pub const POSITION_TOKEN_METADATA_VERSION: u8 = 2;

#[derive(Accounts)]
#[instruction(key_account: Pubkey)]
pub struct CreateEntry<'info> {
    /// The address paying the rent for the account
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The authority that must sign to make this change
    pub authority: Signer<'info>,

    /// The airspace that the entry belongs to
    #[account(
        constraint = airspace.authority == authority.key(),
    )]
    pub airspace: Box<Account<'info, Airspace>>,

    /// The account containing the metadata for the key
    /// CHECK: The account we write metadata to, it can have arbitrary data, and it is up to the authority to not corrupt it
    #[account(init,
              seeds = [airspace.key().as_ref(), key_account.as_ref()],
              bump,
              space = 8, // Use the minimum size, rely on reallocating data
              payer = payer
    )]
    pub metadata_account: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(key_account: Pubkey)]
pub struct SetEntry<'info> {
    /// The address paying the rent for the account if additional rent is required
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The authority that must sign to make this change
    pub authority: Signer<'info>,

    /// The airspace that the entry belongs to]
    #[account(
        constraint = airspace.authority == authority.key(),
    )]
    pub airspace: Box<Account<'info, Airspace>>,

    /// The account containing the metadata to change
    /// CHECK: The seeds validate that this account belong to the airspace
    #[account(mut,
        seeds = [airspace.key().as_ref(), key_account.as_ref()],
        bump,
    )]
    pub metadata_account: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(key_account: Pubkey)]
pub struct RemoveEntry<'info> {
    /// The address receiving the rent
    /// CHECK: Expected to be a wallet, only mutating to receive rent
    #[account(mut)]
    pub receiver: AccountInfo<'info>,

    /// The authority that must sign to make this change
    pub authority: Signer<'info>,

    /// The airspace that the entry belongs to
    #[account(
        constraint = airspace.authority == authority.key(),
    )]
    pub airspace: Box<Account<'info, Airspace>>,

    /// The account containing the metadata to change
    /// CHECK: This is safe because we can only mutate accounts that we own, and we are
    /// closing this account. The metadata program only has entries, so this is then
    /// presumed to always be an entry. The risk could be if we close a different type
    /// of account unintentionally.
    #[account(mut,
        seeds = [airspace.key().as_ref(), key_account.as_ref()],
        bump,
    )]
    pub metadata_account: AccountInfo<'info>,
}

#[program]
mod metadata {
    use anchor_lang::system_program;

    use super::*;

    /// Create an entry
    ///
    /// The key_account is used to validate the metadata PDA
    #[allow(unused_variables)]
    pub fn create_entry(ctx: Context<CreateEntry>, key_account: Pubkey, space: u64) -> Result<()> {
        // no op
        Ok(())
    }

    /// Set an entry, increasing space as necessary
    ///
    /// The key_account is used to validate the metadata PDA
    #[allow(unused)]
    pub fn set_entry(
        ctx: Context<SetEntry>,
        key_account: Pubkey,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        // Check if the metadata account needs to be resized
        let metadata_account = ctx.accounts.metadata_account.to_account_info();

        let offset: usize = offset as usize;
        // Check for overflow
        let new_len = offset
            .checked_add(data.len())
            .ok_or(ErrorCode::ConstraintSpace)?;
        let curr_len = metadata_account.data_len();
        if curr_len < new_len {
            // We need to realloc
            let rent = Rent::get()?;
            let transfer_amount = rent
                .minimum_balance(new_len)
                .saturating_sub(metadata_account.lamports());

            if transfer_amount > 0 {
                anchor_lang::system_program::transfer(
                    CpiContext::new(
                        ctx.accounts.system_program.to_account_info(),
                        anchor_lang::system_program::Transfer {
                            from: ctx.accounts.payer.to_account_info(),
                            to: metadata_account.clone(),
                        },
                    ),
                    transfer_amount,
                )?;
            }

            metadata_account.realloc(new_len, false)?;
        }

        let mut metadata = ctx.accounts.metadata_account.data.borrow_mut();

        metadata[offset..offset + data.len()].copy_from_slice(&data);
        Ok(())
    }

    /// Remove an entry.
    ///
    /// The key_account is used to validate the metadata PDA
    #[allow(unused)]
    pub fn remove_entry(ctx: Context<RemoveEntry>, key_account: Pubkey) -> Result<()> {
        // We manually close the account by draining its lamports and resetting the discriminator to zeroes.
        // We could reset all bytes, but as the account is being closed, this is unnecessary.
        let mut source = ctx.accounts.metadata_account.try_borrow_mut_lamports()?;
        let mut dest = ctx.accounts.receiver.try_borrow_mut_lamports()?;

        **dest = dest.checked_add(**source).unwrap();
        **source = 0;

        ctx.accounts.metadata_account.assign(&system_program::ID);
        ctx.accounts.metadata_account.realloc(0, false)?;

        Ok(())
    }
}

/// Description of the token's usage
#[derive(AnchorSerialize, AnchorDeserialize, Eq, PartialEq, Clone, Copy, Debug)]
pub enum TokenKind {
    /// The token has no value within the margin system
    NonCollateral,

    /// The token can be used as collateral
    Collateral,

    /// The token represents a debt that needs to be repaid
    Claim,

    /// The token balance is managed by a trusted adapter to represent the amount of collateral custodied by that adapter.
    /// The token account is owned by the adapter. Collateral is accessed through instructions to the adapter.
    AdapterCollateral,
}

#[derive(AnchorSerialize, AnchorDeserialize, Eq, PartialEq, Clone, Copy, Debug)]
pub enum PositionOwner {
    MarginAccount,
    Adapter,
}

impl Default for TokenKind {
    fn default() -> TokenKind {
        Self::NonCollateral
    }
}

/// A metadata account referencing information about a position token
#[account]
#[derive(Debug, Eq, PartialEq)]
pub struct PositionTokenMetadata {
    /// The airspace that the entry belongs to
    pub airspace: Pubkey,

    /// The mint for the position token
    pub position_token_mint: Pubkey,

    /// The underlying token represented by this position
    pub underlying_token_mint: Pubkey,

    /// The adapter program in control of this position
    pub adapter_program: Pubkey,

    /// The token program of this position
    pub token_program: Pubkey,

    /// Description of this token
    pub token_kind: TokenKind,

    /// A modifier to adjust the token value, based on the kind of token
    pub value_modifier: u16,

    /// The maximum staleness (seconds) that's acceptable for balances of this token
    pub max_staleness: u64,

    /// Token features
    /// NOTE: this is a breaking feature as the account size changes. The metadata program
    /// should however increase the account size when updating.
    /// It is readers that will error out due to the size mismatch. We will prepare migrations.
    pub token_features: u16,

    pub version: u8,

    /// Reserved bytes
    pub reserved: [u8; 64],
}

impl Default for PositionTokenMetadata {
    fn default() -> Self {
        Self {
            airspace: Default::default(),
            position_token_mint: Default::default(),
            underlying_token_mint: Default::default(),
            adapter_program: Default::default(),
            token_program: Default::default(),
            token_kind: Default::default(),
            value_modifier: Default::default(),
            max_staleness: Default::default(),
            token_features: Default::default(),
            version: Default::default(),
            reserved: [0; 64],
        }
    }
}

/// An account that references information about a token's price oracle
#[account]
#[derive(Default, Debug, Eq, PartialEq)]
pub struct TokenMetadata {
    /// The airspace that the entry belongs to
    pub airspace: Pubkey,

    /// The address of the mint for the token being referenced
    pub token_mint: Pubkey,

    /// Details about the price oracle
    pub token_price_oracle: TokenPriceOracle,
    // Note, we don't need to change this for now, but we should add a version at the next
    // opportune moment.
}
