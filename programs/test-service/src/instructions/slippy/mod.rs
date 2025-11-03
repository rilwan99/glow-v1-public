//! Slippy is a swap pool that swaps tokens with a guaranteed slippage.
//!
//! We use it to test liquidations by simulating different equity loss scenarios.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::state::SlippyPool;

#[derive(Accounts)]
pub struct InitSlippyPool<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        init,
        seeds = [
            mint_a.key().as_ref(),
            mint_b.key().as_ref(),
            b"slippy-pool",
        ],
        space = std::mem::size_of::<SlippyPool>() + 8,
        bump,
        payer = payer,
    )]
    pub slippy: Box<Account<'info, SlippyPool>>,

    pub mint_a: Box<InterfaceAccount<'info, Mint>>,
    pub mint_b: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        init,
        seeds = [
            slippy.key().as_ref(),
            mint_a.key().as_ref(),
        ],
        bump,
        payer = payer,
        token::mint = mint_a,
        token::authority = slippy,
        token::token_program = token_program_a,
    )]
    pub vault_a: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        init,
        seeds = [
            slippy.key().as_ref(),
            mint_b.key().as_ref(),
        ],
        bump,
        payer = payer,
        token::mint = mint_b,
        token::authority = slippy,
        token::token_program = token_program_b,
    )]
    pub vault_b: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program_a: Interface<'info, TokenInterface>,
    pub token_program_b: Interface<'info, TokenInterface>,

    pub system_program: Program<'info, System>,
}

pub fn init_slippy_pool_handler(ctx: Context<InitSlippyPool>) -> Result<()> {
    let slippy = &mut ctx.accounts.slippy;
    slippy.mint_a = ctx.accounts.mint_a.key();
    slippy.mint_b = ctx.accounts.mint_b.key();
    slippy.mint_a_token_program = ctx.accounts.token_program_a.key();
    slippy.mint_b_token_program = ctx.accounts.token_program_b.key();
    slippy.vault_a = ctx.accounts.vault_a.key();
    slippy.vault_b = ctx.accounts.vault_b.key();
    slippy.seed = [ctx.bumps.slippy];

    // We can fund a slippy pool by simply depositing tokens into its vaults.

    Ok(())
}

#[derive(Accounts)]
pub struct SwapSlippyPool<'info> {
    #[account(mut)]
    pub signer: Signer<'info>,
    #[account(
        seeds = [
            mint_a.key().as_ref(),
            mint_b.key().as_ref(),
            b"slippy-pool",
        ],
        bump,
    )]
    pub slippy: Account<'info, SlippyPool>,

    pub mint_a: InterfaceAccount<'info, Mint>,
    pub mint_b: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        seeds = [
            slippy.key().as_ref(),
            mint_a.key().as_ref(),
        ],
        bump,
        token::mint = mint_a,
        token::authority = slippy,
        token::token_program = token_program_a,
    )]
    pub vault_a: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [
            slippy.key().as_ref(),
            mint_b.key().as_ref(),
        ],
        bump,
        token::mint = mint_b,
        token::authority = slippy,
        token::token_program = token_program_b,
    )]
    pub vault_b: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = mint_a,
        token::authority = signer,
        token::token_program = token_program_a,
    )]
    pub signer_token_a: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = mint_b,
        token::authority = signer,
        token::token_program = token_program_b,
    )]
    pub signer_token_b: InterfaceAccount<'info, TokenAccount>,

    pub token_program_a: Interface<'info, TokenInterface>,
    pub token_program_b: Interface<'info, TokenInterface>,
}

/// Swap tokens from the slippy pool, specifying a swap price and simulated slippage
pub fn swap_slippy_pool_handler(
    ctx: Context<SwapSlippyPool>,
    amount_in: u64,
    a_to_b: bool,
    a_to_b_exchange_rate: f64,
    slippage: f64, // in percentage
) -> Result<()> {
    let signer_seeds: &[&[&[u8]]] = &[&[
        ctx.accounts.slippy.mint_a.as_ref(),
        ctx.accounts.slippy.mint_b.as_ref(),
        b"slippy-pool",
        ctx.accounts.slippy.seed.as_ref(),
    ]];
    if a_to_b {
        let total_value = a_to_b_exchange_rate
            * (amount_in as f64 / 10f64.powi(ctx.accounts.mint_a.decimals as i32));
        let tokens_out = total_value * 10f64.powi(ctx.accounts.mint_b.decimals as i32);
        let tokens_out_slippage = tokens_out * (1.0 - slippage);
        anchor_spl::token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program_a.to_account_info(),
                anchor_spl::token_interface::TransferChecked {
                    from: ctx.accounts.signer_token_a.to_account_info(),
                    mint: ctx.accounts.mint_a.to_account_info(),
                    to: ctx.accounts.vault_a.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
            ),
            amount_in,
            ctx.accounts.mint_a.decimals,
        )?;
        anchor_spl::token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program_b.to_account_info(),
                anchor_spl::token_interface::TransferChecked {
                    from: ctx.accounts.vault_b.to_account_info(),
                    mint: ctx.accounts.mint_b.to_account_info(),
                    to: ctx.accounts.signer_token_b.to_account_info(),
                    authority: ctx.accounts.slippy.to_account_info(),
                },
                signer_seeds,
            ),
            tokens_out_slippage as u64,
            ctx.accounts.mint_b.decimals,
        )?;
    } else {
        let total_value = (1.0 / a_to_b_exchange_rate)
            * (amount_in as f64 / 10f64.powi(ctx.accounts.mint_b.decimals as i32));
        let tokens_out = total_value * 10f64.powi(ctx.accounts.mint_a.decimals as i32);
        let tokens_out_slippage = tokens_out * (1.0 - slippage);
        anchor_spl::token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program_b.to_account_info(),
                anchor_spl::token_interface::TransferChecked {
                    from: ctx.accounts.signer_token_b.to_account_info(),
                    mint: ctx.accounts.mint_b.to_account_info(),
                    to: ctx.accounts.vault_b.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
            ),
            amount_in,
            ctx.accounts.mint_b.decimals,
        )?;
        anchor_spl::token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program_a.to_account_info(),
                anchor_spl::token_interface::TransferChecked {
                    from: ctx.accounts.vault_a.to_account_info(),
                    mint: ctx.accounts.mint_a.to_account_info(),
                    to: ctx.accounts.signer_token_a.to_account_info(),
                    authority: ctx.accounts.slippy.to_account_info(),
                },
                signer_seeds,
            ),
            tokens_out_slippage as u64,
            ctx.accounts.mint_a.decimals,
        )?;
    }

    Ok(())
}
