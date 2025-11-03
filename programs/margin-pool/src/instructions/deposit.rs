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

use std::ops::Deref;

use anchor_lang::prelude::*;
use anchor_spl::token::{self};
use anchor_spl::token_2022::MintTo;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};
use glow_margin::{MarginAccount, TokenFeatures};
use glow_metadata::PositionTokenMetadata;
use glow_program_common::token_change::{ChangeKind, TokenChange};

use crate::ErrorCode;
use crate::{events, state::*, Amount};

#[derive(Accounts)]
pub struct Deposit<'info> {
    /// The pool to deposit into
    ///
    /// NOTE: It is a known limitation that any user can deposit into a restricted airspace pool.
    /// This will be fixed in future.
    #[account(mut,
              has_one = vault,
              has_one = deposit_note_mint)]
    pub margin_pool: Account<'info, MarginPool>,

    /// The vault for the pool, where tokens are held
    /// CHECK: Owned by the pool as its signer
    #[account(mut)]
    pub vault: UncheckedAccount<'info>,

    /// The mint for the deposit notes
    /// CHECK: Owned by the pool as its authority
    #[account(mut)]
    pub deposit_note_mint: UncheckedAccount<'info>,

    /// The address with authority to deposit the tokens
    pub depositor: Signer<'info>,

    /// The source of the tokens to be deposited
    #[account(
        mut,
        token::mint = source_mint,
        token::token_program = mint_token_program,
        token::authority = depositor
    )]
    pub source: InterfaceAccount<'info, TokenAccount>,

    pub source_mint: InterfaceAccount<'info, Mint>,

    /// The destination of the deposit notes
    ///
    /// NOTE: in restricted airspaces, it would be a violation if the destination was not owned by an account
    /// that has an airspace permit (either the wallet/account or its margin account).
    #[account(
        mut,
        token::token_program = pool_token_program,
        token::mint = deposit_note_mint
    )]
    pub destination: InterfaceAccount<'info, TokenAccount>,

    pub mint_token_program: Interface<'info, TokenInterface>,
    pub pool_token_program: Interface<'info, TokenInterface>,

    /// Pool token configuration to enable checking token restrictions
    #[account(
        constraint = deposit_metadata.position_token_mint == deposit_note_mint.key(),
    )]
    pub deposit_metadata: Box<Account<'info, PositionTokenMetadata>>,

    /// Margin account needed if depositing into restricted pools.
    pub optional_margin_account: Option<AccountLoader<'info, MarginAccount>>,
}

impl<'info> Deposit<'info> {
    fn transfer_source_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        CpiContext::new(
            self.mint_token_program.to_account_info(),
            TransferChecked {
                to: self.vault.to_account_info(),
                from: self.source.to_account_info(),
                authority: self.depositor.to_account_info(),
                mint: self.source_mint.to_account_info(),
            },
        )
    }

    /// Mint notes using token-2022
    fn mint_note_context(&self) -> CpiContext<'_, '_, '_, 'info, MintTo<'info>> {
        CpiContext::new(
            self.pool_token_program.to_account_info(),
            MintTo {
                to: self.destination.to_account_info(),
                mint: self.deposit_note_mint.to_account_info(),
                authority: self.margin_pool.to_account_info(),
            },
        )
    }
}

pub fn deposit_handler(ctx: Context<Deposit>, change_kind: ChangeKind, amount: u64) -> Result<()> {
    let change = TokenChange {
        kind: change_kind,
        tokens: amount,
    };

    let pool = &mut ctx.accounts.margin_pool;
    let clock = Clock::get()?;

    // Make sure interest accrual is up-to-date
    if !pool.accrue_interest(clock.unix_timestamp) {
        msg!("interest accrual is too far behind");
        return Err(ErrorCode::InterestAccrualBehind.into());
    }

    // If this pool's token is restricted, check that the signer is a margin account.
    // We check this by using the margin account's discriminator and by verifying its owner.
    if TokenFeatures::from_bits_retain(ctx.accounts.deposit_metadata.token_features)
        .contains(TokenFeatures::RESTRICTED)
    {
        let margin_account = ctx
            .accounts
            .optional_margin_account
            .as_ref()
            .ok_or(error!(ErrorCode::PoolPermissionDenied))?;
        let margin_account_address = margin_account.key();
        let margin_account = margin_account.load()?;
        require!(
            margin_account.owner == ctx.accounts.depositor.key(),
            ErrorCode::PoolPermissionDenied
        );
        require!(
            margin_account.airspace == pool.airspace,
            ErrorCode::PoolPermissionDenied,
        );
        // The destination tokens should go to a token account owned by the margin account.
        require!(
            ctx.accounts.destination.owner == margin_account_address,
            ErrorCode::PoolPermissionDenied
        );
        // The token features should be compatible with the margin account.
        // Nothing stops a user from misusing this instruction by:
        // - creating a token account (that's not an ATA) owned by the margin account,
        // - directly invoking this instruction and bypassing AdapterInvoke (and bypassing margin's feature validation)
        // - depositing tokens into this token account.
        // The only further restriction we can add is to ensure that the margin account's features are
        // compatible with the token.
        let token_features =
            TokenFeatures::from_bits_truncate(ctx.accounts.deposit_metadata.token_features);
        require!(
            margin_account
                .features
                .are_token_features_compatible(token_features)?,
            ErrorCode::PoolPermissionDenied
        )
    }

    // Amount the user desires to deposit
    let source_token_amount = token::accessor::amount(&ctx.accounts.source.to_account_info())?;
    let source_balance =
        pool.convert_amount(Amount::tokens(source_token_amount), PoolAction::Deposit)?;

    let destination_pool_notes =
        token::accessor::amount(&ctx.accounts.destination.to_account_info())?;
    let destination_balance =
        pool.convert_amount(Amount::notes(destination_pool_notes), PoolAction::Deposit)?;
    let deposit_amount = pool.calculate_full_amount(
        source_balance,
        destination_balance,
        change,
        PoolAction::Deposit,
    )?;
    pool.deposit(&deposit_amount)?;

    let pool = &ctx.accounts.margin_pool;
    let signer = [&pool.signer_seeds()?[..]];

    token_interface::transfer_checked(
        ctx.accounts.transfer_source_context(),
        deposit_amount.tokens,
        ctx.accounts.source_mint.decimals,
    )?;
    token_interface::mint_to(
        ctx.accounts.mint_note_context().with_signer(&signer),
        deposit_amount.notes,
    )?;

    emit!(events::Deposit {
        margin_pool: ctx.accounts.margin_pool.key(),
        user: ctx.accounts.depositor.key(),
        source: ctx.accounts.source.key(),
        destination: ctx.accounts.destination.key(),
        deposit_tokens: deposit_amount.tokens,
        deposit_notes: deposit_amount.notes,
        summary: pool.deref().into(),
    });

    Ok(())
}
