use anchor_lang::prelude::*;

use crate::{seeds::TEST_SERVICE_AUTHORITY, state::TestServiceAuthority};

#[derive(Accounts)]
pub struct InitTestServiceAuthority<'info> {
    #[account(mut)]
    pub signer: Signer<'info>,

    #[account(
        init,
        seeds = [
            TEST_SERVICE_AUTHORITY
        ],
        bump,
        payer = signer,
        space = 8 + std::mem::size_of::<TestServiceAuthority>(),
    )]
    pub test_service_authority: Account<'info, TestServiceAuthority>,

    system_program: Program<'info, System>,
}

pub fn init_test_service_authority_handler(ctx: Context<InitTestServiceAuthority>) -> Result<()> {
    let authority = &mut ctx.accounts.test_service_authority;

    authority.seed = [ctx.bumps.test_service_authority];

    Ok(())
}
