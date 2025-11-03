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

use anchor_lang::{prelude::*, Discriminator};
use anchor_spl::token_2022::{Burn, TransferChecked};
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};
use anchor_spl::{token, token_interface};
use glow_margin::{AccountConstraints, MarginAccount};
use glow_program_common::token_change::{ChangeKind, TokenChange};

use crate::{events, state::*, ErrorCode};

#[derive(Accounts)]
pub struct Withdraw<'info> {
    /// The address with authority to withdraw the deposit
    ///
    /// For margin accounts, the signer is the margin account.
    /// For direct deposits, the signer is the owner/wallet.
    pub depositor: Signer<'info>,

    /// The pool to withdraw from
    #[account(mut,
              has_one = vault,
              has_one = deposit_note_mint)]
    pub margin_pool: Account<'info, MarginPool>,

    /// The vault for the pool, where tokens are held
    /// CHECK: Checked to belong to the margin pool
    #[account(mut)]
    pub vault: AccountInfo<'info>,

    /// The mint for the deposit notes
    /// CHECK: Checked to belong to the margin pool
    #[account(mut)]
    pub deposit_note_mint: UncheckedAccount<'info>,

    /// The source of the deposit notes to be redeemed
    /// CHECK: Validated by the token program to be owned by the signer
    #[account(mut)]
    pub source: UncheckedAccount<'info>,

    /// The destination of the tokens withdrawn
    ///
    /// If the signer is a margin account with withdrawal constraints set
    /// (`AccountConstraints::DENY_WITHDRAWALS`), the destination must be the
    /// margin account's associated token account (ATA) for `token_mint`.
    ///
    /// Otherwise, if the signer is a margin account, the destination can be
    /// owned by either the margin account itself or the margin account owner.
    /// If the signer is not a margin account, the destination must be owned by
    /// the signer.
    #[account(
        mut,
        token::mint = token_mint,
        token::token_program = mint_token_program,
    )]
    pub destination: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        associated_token::mint = token_mint,
        associated_token::token_program = mint_token_program,
        associated_token::authority = depositor,
    )]
    pub destination_ata: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_mint: Box<InterfaceAccount<'info, Mint>>,

    pub mint_token_program: Interface<'info, TokenInterface>,
    pub pool_token_program: Interface<'info, TokenInterface>,
}

impl<'info> Withdraw<'info> {
    fn transfer_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        CpiContext::new(
            self.mint_token_program.to_account_info(),
            TransferChecked {
                to: self.destination.to_account_info(),
                from: self.vault.to_account_info(),
                authority: self.margin_pool.to_account_info(),
                mint: self.token_mint.to_account_info(),
            },
        )
    }

    fn burn_note_context(&self) -> CpiContext<'_, '_, '_, 'info, Burn<'info>> {
        CpiContext::new(
            self.pool_token_program.to_account_info(),
            Burn {
                from: self.source.to_account_info(),
                mint: self.deposit_note_mint.to_account_info(),
                authority: self.depositor.to_account_info(),
            },
        )
    }
}

pub fn withdraw_handler(
    ctx: Context<Withdraw>,
    change_kind: ChangeKind,
    amount: u64,
) -> Result<()> {
    // Check the destination's valid owner(s)
    let destination_authority = ctx.accounts.destination.owner;
    let depositor_address = ctx.accounts.depositor.key();
    let depositor = ctx.accounts.depositor.to_account_info();
    if !depositor.data_is_empty() && depositor.owner == &glow_margin::ID {
        // Might be a margin account, check discriminator
        let data = depositor.try_borrow_data()?;
        if &data[0..8] == &MarginAccount::DISCRIMINATOR {
            let margin_account: &MarginAccount =
                bytemuck::try_from_bytes(&data[8..]).map_err(|e| {
                    msg!("Error reading margin account {:?}", e);
                    crate::ErrorCode::InvalidWithdrawalAuthority
                })?;
            let denies_withdrawals = margin_account
                .constraints
                .contains(AccountConstraints::DENY_WITHDRAWALS)
                || margin_account
                    .constraints
                    .contains(AccountConstraints::DENY_TRANSFERS);
            if denies_withdrawals {
                // Operator-style accounts may only withdraw to their own ATA
                require!(
                    ctx.accounts.destination.key() == ctx.accounts.destination_ata.key(),
                    crate::ErrorCode::InvalidWithdrawalAuthority
                );
            } else {
                // Regular margin accounts may withdraw to either their owner wallet's ATA or their own ATA
                require!(
                    destination_authority == margin_account.owner
                        || destination_authority == depositor_address,
                    crate::ErrorCode::InvalidWithdrawalAuthority
                );
            }
        } else {
            require!(
                destination_authority == depositor_address,
                crate::ErrorCode::InvalidWithdrawalAuthority
            );
        }
    } else {
        require!(
            destination_authority == depositor_address,
            crate::ErrorCode::InvalidWithdrawalAuthority
        );
    }
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

    let pool_notes = token::accessor::amount(&ctx.accounts.source.to_account_info())?;
    let destination_token_balance =
        token::accessor::amount(&ctx.accounts.destination.to_account_info())?;
    let pool_full_amount =
        pool.convert_amount(crate::Amount::notes(pool_notes), PoolAction::Withdraw)?;
    let withdraw_amount = pool.calculate_full_amount(
        pool_full_amount,
        FullAmount {
            tokens: destination_token_balance,
            notes: destination_token_balance,
        },
        change,
        PoolAction::Withdraw,
    )?;
    pool.withdraw(&withdraw_amount)?;

    let pool = &ctx.accounts.margin_pool;
    let signer = [&pool.signer_seeds()?[..]];

    token_interface::transfer_checked(
        ctx.accounts.transfer_context().with_signer(&signer),
        withdraw_amount.tokens,
        ctx.accounts.token_mint.decimals,
    )?;
    token_interface::burn(ctx.accounts.burn_note_context(), withdraw_amount.notes)?;

    emit!(events::Withdraw {
        margin_pool: ctx.accounts.margin_pool.key(),
        user: ctx.accounts.depositor.key(),
        source: ctx.accounts.source.key(),
        destination: ctx.accounts.destination.key(),
        withdraw_tokens: withdraw_amount.tokens,
        withdraw_notes: withdraw_amount.notes,
        summary: pool.deref().into(),
    });

    Ok(())
}
