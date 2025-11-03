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

use anchor_lang::{
    prelude::*,
    system_program::{create_account, CreateAccount},
    Discriminator,
};
use anchor_spl::{
    token_2022::{
        spl_token_2022::extension::{ExtensionType, StateWithExtensions},
        Token2022,
    },
    token_interface::{Mint, TokenInterface},
};

use glow_airspace::state::Airspace;
use glow_metadata::{
    cpi::accounts::{CreateEntry, SetEntry},
    POSITION_TOKEN_METADATA_VERSION,
};
use glow_metadata::{program::Metadata, PositionTokenMetadata, TokenKind, TokenMetadata};
use glow_program_common::oracle::TokenPriceOracle;

use crate::{
    current_pool_version,
    events::{self, MarginPoolSummary},
    state::*,
    util::validate_mint_extension,
};

#[derive(Accounts)]
pub struct CreatePool<'info> {
    /// The authority to create pools, which must be the airspace authority
    pub authority: Signer<'info>,

    /// The airspace that the pool is being registered in
    #[account(
      constraint = airspace.authority == authority.key(),
    )]
    pub airspace: Box<Account<'info, Airspace>>,

    /// The fee owner, which must sign to prove existence
    pub fee_owner: Signer<'info>,

    /// The payer of rent for new accounts
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The pool to be created
    #[account(
        mut,
        seeds = [
          airspace.key().as_ref(),
          token_mint.key().as_ref()
          ],
        bump,
    )]
    pub margin_pool: UncheckedAccount<'info>,

    /// The token account holding the pool's deposited funds
    #[account(mut,
              seeds = [
                margin_pool.key().as_ref(),
                b"vault".as_ref()
              ],
              bump,
    )]
    pub vault: UncheckedAccount<'info>,

    /// The mint for deposit notes
    #[account(init,
              seeds = [
                margin_pool.key().as_ref(),
                b"deposit-notes".as_ref()
              ],
              bump,
              mint::decimals = token_mint.decimals,
              mint::authority = margin_pool,
              mint::token_program = pool_token_program,
              payer = payer
    )]
    pub deposit_note_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The mint for loan notes
    #[account(init,
              seeds = [
                margin_pool.key().as_ref(),
                b"loan-notes".as_ref()
              ],
              bump,
              mint::decimals = token_mint.decimals,
              mint::authority = margin_pool,
              mint::token_program = pool_token_program,
              payer = payer
    )]
    pub loan_note_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The mint for the token being custodied by the pool
    pub token_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The deposit mint account where fees are transferred to.
    /// This should be owned by a fee owner, which can be the airspace authority
    #[account(mut,
      seeds = [
        margin_pool.key().as_ref(),
        // Note: the intention is to allow changing a pool fee owner in future.
        fee_owner.key().as_ref(),
        crate::seeds::FEE_DESTINATION,
      ],
      bump,
    )]
    pub fee_destination: UncheckedAccount<'info>,

    /// CHECK:
    #[account(mut)]
    pub token_metadata: UncheckedAccount<'info>,

    /// CHECK:
    #[account(mut)]
    pub deposit_note_metadata: UncheckedAccount<'info>,

    /// CHECK:
    #[account(mut)]
    pub loan_note_metadata: UncheckedAccount<'info>,

    pub mint_token_program: Interface<'info, TokenInterface>,
    pub pool_token_program: Program<'info, Token2022>,
    pub metadata_program: Program<'info, Metadata>,
    pub system_program: Program<'info, System>,
}

impl<'info> CreatePool<'info> {
    fn create_token_metadata_context(&self) -> CpiContext<'_, '_, '_, 'info, CreateEntry<'info>> {
        CpiContext::new(
            self.metadata_program.to_account_info(),
            CreateEntry {
                airspace: self.airspace.to_account_info(),
                metadata_account: self.token_metadata.to_account_info(),
                authority: self.authority.to_account_info(),
                payer: self.payer.to_account_info(),
                system_program: self.system_program.to_account_info(),
            },
        )
    }

    fn set_token_metadata_context(&self) -> CpiContext<'_, '_, '_, 'info, SetEntry<'info>> {
        CpiContext::new(
            self.metadata_program.to_account_info(),
            SetEntry {
                airspace: self.airspace.to_account_info(),
                metadata_account: self.token_metadata.to_account_info(),
                authority: self.authority.to_account_info(),
                payer: self.payer.to_account_info(),
                system_program: self.system_program.to_account_info(),
            },
        )
    }

    fn create_deposit_metadata_context(&self) -> CpiContext<'_, '_, '_, 'info, CreateEntry<'info>> {
        CpiContext::new(
            self.metadata_program.to_account_info(),
            CreateEntry {
                airspace: self.airspace.to_account_info(),
                metadata_account: self.deposit_note_metadata.to_account_info(),
                authority: self.authority.to_account_info(),
                payer: self.payer.to_account_info(),
                system_program: self.system_program.to_account_info(),
            },
        )
    }

    fn set_deposit_metadata_context(&self) -> CpiContext<'_, '_, '_, 'info, SetEntry<'info>> {
        CpiContext::new(
            self.metadata_program.to_account_info(),
            SetEntry {
                airspace: self.airspace.to_account_info(),
                metadata_account: self.deposit_note_metadata.to_account_info(),
                authority: self.authority.to_account_info(),
                payer: self.payer.to_account_info(),
                system_program: self.system_program.to_account_info(),
            },
        )
    }

    fn create_loan_metadata_context(&self) -> CpiContext<'_, '_, '_, 'info, CreateEntry<'info>> {
        CpiContext::new(
            self.metadata_program.to_account_info(),
            CreateEntry {
                airspace: self.airspace.to_account_info(),
                metadata_account: self.loan_note_metadata.to_account_info(),
                authority: self.authority.to_account_info(),
                payer: self.payer.to_account_info(),
                system_program: self.system_program.to_account_info(),
            },
        )
    }

    fn set_loan_metadata_context(&self) -> CpiContext<'_, '_, '_, 'info, SetEntry<'info>> {
        CpiContext::new(
            self.metadata_program.to_account_info(),
            SetEntry {
                airspace: self.airspace.to_account_info(),
                metadata_account: self.loan_note_metadata.to_account_info(),
                authority: self.authority.to_account_info(),
                payer: self.payer.to_account_info(),
                system_program: self.system_program.to_account_info(),
            },
        )
    }

    fn create_token_account(
        &self,
        account: AccountInfo<'info>,
        mint: AccountInfo<'info>,
        authority: AccountInfo<'info>,
        token_program: AccountInfo<'info>,
        seeds: &[&[&[u8]]],
    ) -> Result<()> {
        use anchor_spl::token_2022::spl_token_2022::extension::BaseStateWithExtensions;
        use anchor_spl::token_2022::spl_token_2022::state::Mint as Mint2022;

        assert_eq!(mint.owner, token_program.key);

        let ctx = CpiContext::new(
            self.system_program.to_account_info(),
            CreateAccount {
                from: self.payer.to_account_info(),
                to: account.to_account_info(),
            },
        )
        .with_signer(seeds);
        let space = match *mint.owner {
            anchor_spl::token::ID => anchor_spl::token::TokenAccount::LEN,
            anchor_spl::token_2022::ID => {
                let mint_data = mint.try_borrow_data()?;
                let mint_state = StateWithExtensions::<Mint2022>::unpack(&mint_data)?;
                let mint_extensions = mint_state.get_extension_types()?;
                let required_extensions =
                    ExtensionType::get_required_init_account_extensions(&mint_extensions);
                ExtensionType::try_calculate_account_len::<
                    anchor_spl::token_2022::spl_token_2022::state::Account,
                >(&required_extensions)?
            }
            _ => panic!(),
        };
        let lamports = Rent::get()?.minimum_balance(space);
        create_account(ctx, lamports, space as _, mint.owner)?;

        let accounts = anchor_spl::token_interface::InitializeAccount3 {
            account,
            mint,
            authority,
        };
        let ctx = CpiContext::new(token_program, accounts);
        anchor_spl::token_interface::initialize_account3(ctx)
    }
}

/// Create a margin pool.
///
/// NOTE: We split initializing the pool and tokens from the mints as the instruction was
/// exceeding the stack size limit. As of anchor_lang 0.30 (and solana 1.18), trying to
/// initialize 3+ accounts with `#[accounts(init)]` will exceed the stack size limit.
/// Thus our solution was to split this up.
pub fn create_pool_handler(ctx: Context<CreatePool>) -> Result<()> {
    // If invalid mint extension, bail early
    validate_mint_extension(ctx.accounts.token_mint.to_account_info())?;

    // Create vault
    ctx.accounts.create_token_account(
        ctx.accounts.vault.to_account_info(),
        ctx.accounts.token_mint.to_account_info(),
        ctx.accounts.margin_pool.to_account_info(),
        ctx.accounts.mint_token_program.to_account_info(),
        &[&[
            ctx.accounts.margin_pool.key().as_ref(),
            b"vault",
            &[ctx.bumps.vault][..],
        ]],
    )?;

    // Create fee destination
    ctx.accounts.create_token_account(
        ctx.accounts.fee_destination.to_account_info(),
        ctx.accounts.deposit_note_mint.to_account_info(),
        ctx.accounts.fee_owner.to_account_info(),
        ctx.accounts.pool_token_program.to_account_info(),
        &[&[
            ctx.accounts.margin_pool.key().as_ref(),
            ctx.accounts.fee_owner.key().as_ref(),
            crate::seeds::FEE_DESTINATION,
            &[ctx.bumps.fee_destination][..],
        ]],
    )?;

    // Create space for the pool
    {
        let a = ctx.accounts.airspace.key();
        let b = ctx.accounts.token_mint.key();
        let signer_seeds: &[&[u8]] = &[a.as_ref(), b.as_ref(), &[ctx.bumps.margin_pool]];
        let signer_seeds = &[signer_seeds];
        let init_ctx = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            CreateAccount {
                from: ctx.accounts.payer.to_account_info(),
                to: ctx.accounts.margin_pool.to_account_info(),
            },
        )
        .with_signer(signer_seeds);
        let space = 8 + std::mem::size_of::<MarginPool>();
        let lamports = Rent::get()?.minimum_balance(space);
        create_account(init_ctx, lamports, space as _, &crate::ID)?;
    }

    let mut pool = MarginPool::default();
    let pool_version = current_pool_version();

    pool.version = pool_version;
    pool.address = ctx.accounts.margin_pool.key();
    pool.airspace = ctx.accounts.airspace.key();
    pool.pool_bump[0] = ctx.bumps.margin_pool;
    pool.token_mint = ctx.accounts.token_mint.key();
    pool.vault = ctx.accounts.vault.key();
    pool.deposit_note_mint = ctx.accounts.deposit_note_mint.key();
    pool.loan_note_mint = ctx.accounts.loan_note_mint.key();
    pool.fee_destination = ctx.accounts.fee_destination.key();

    let clock = Clock::get()?;
    pool.accrued_until = clock.unix_timestamp;

    // Save pool
    let mut data = ctx.accounts.margin_pool.try_borrow_mut_data()?;
    data[0..8].copy_from_slice(&MarginPool::discriminator());
    pool.serialize(&mut &mut data[8..])?;

    emit!(events::PoolCreated {
        fee_destination: ctx.accounts.fee_destination.key(),
        margin_pool: ctx.accounts.margin_pool.key(),
        vault: ctx.accounts.vault.key(),
        deposit_note_mint: ctx.accounts.deposit_note_mint.key(),
        loan_note_mint: ctx.accounts.loan_note_mint.key(),
        token_mint: ctx.accounts.token_mint.key(),
        authority: ctx.accounts.authority.key(),
        payer: ctx.accounts.payer.key(),
        summary: MarginPoolSummary::from(&pool),
        version: pool_version,
    });

    // set metadata for the deposit/loan tokens to be used as positions
    let deposit_note_metadata = PositionTokenMetadata {
        airspace: ctx.accounts.airspace.key(),
        underlying_token_mint: ctx.accounts.token_mint.key(),
        position_token_mint: ctx.accounts.deposit_note_mint.key(),
        adapter_program: crate::ID,
        token_program: ctx.accounts.pool_token_program.key(),
        token_kind: TokenKind::NonCollateral,
        value_modifier: 0,
        max_staleness: 5,
        token_features: 0,
        version: POSITION_TOKEN_METADATA_VERSION,
        reserved: [0; 64],
    };

    let loan_note_metadata = PositionTokenMetadata {
        airspace: ctx.accounts.airspace.key(),
        underlying_token_mint: ctx.accounts.token_mint.key(),
        position_token_mint: ctx.accounts.loan_note_mint.key(),
        adapter_program: crate::ID,
        token_program: ctx.accounts.pool_token_program.key(),
        token_kind: TokenKind::Claim,
        value_modifier: 0,
        max_staleness: 5,
        token_features: 0,
        version: POSITION_TOKEN_METADATA_VERSION,
        reserved: [0; 64],
    };

    let token_metadata = TokenMetadata {
        airspace: ctx.accounts.airspace.key(),
        token_mint: ctx.accounts.token_mint.key(),
        token_price_oracle: TokenPriceOracle::default(),
    };

    let mut token_md_data = vec![];
    let mut deposit_md_data = vec![];
    let mut loan_md_data = vec![];

    deposit_note_metadata.try_serialize(&mut deposit_md_data)?;
    loan_note_metadata.try_serialize(&mut loan_md_data)?;
    token_metadata.try_serialize(&mut token_md_data)?;

    glow_metadata::cpi::create_entry(
        ctx.accounts.create_deposit_metadata_context(),
        ctx.accounts.deposit_note_mint.key(),
        deposit_md_data.len().try_into().unwrap(),
    )?;

    glow_metadata::cpi::set_entry(
        ctx.accounts.set_deposit_metadata_context(),
        ctx.accounts.deposit_note_mint.key(),
        0,
        deposit_md_data,
    )?;

    emit!(events::PositionTokenMetadataConfigured {
        requester: ctx.accounts.payer.key(),
        authority: ctx.accounts.authority.key(),
        metadata_account: ctx.accounts.deposit_note_metadata.key(),
        metadata: deposit_note_metadata,
    });

    glow_metadata::cpi::create_entry(
        ctx.accounts.create_loan_metadata_context(),
        ctx.accounts.loan_note_mint.key(),
        loan_md_data.len().try_into().unwrap(),
    )?;

    glow_metadata::cpi::set_entry(
        ctx.accounts.set_loan_metadata_context(),
        ctx.accounts.loan_note_mint.key(),
        0,
        loan_md_data,
    )?;

    emit!(events::PositionTokenMetadataConfigured {
        requester: ctx.accounts.payer.key(),
        authority: ctx.accounts.authority.key(),
        metadata_account: ctx.accounts.loan_note_metadata.key(),
        metadata: loan_note_metadata,
    });

    glow_metadata::cpi::create_entry(
        ctx.accounts.create_token_metadata_context(),
        ctx.accounts.token_mint.key(),
        token_md_data.len().try_into().unwrap(),
    )?;

    glow_metadata::cpi::set_entry(
        ctx.accounts.set_token_metadata_context(),
        ctx.accounts.token_mint.key(),
        0,
        token_md_data,
    )?;

    emit!(events::TokenMetadataConfigured {
        requester: ctx.accounts.payer.key(),
        authority: ctx.accounts.authority.key(),
        metadata_account: ctx.accounts.token_metadata.key(),
        metadata: token_metadata,
    });

    Ok(())
}
