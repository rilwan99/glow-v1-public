use anchor_lang::prelude::*;

/// A [SlippyPool] is a simple swap pool that always honours a swap with a provided
/// loss or gain percentage.
#[account]
pub struct SlippyPool {
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub mint_a_token_program: Pubkey,
    pub mint_b_token_program: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub seed: [u8; 1],
}

/*
let's call the instruction

let slippy = Keypair::new();
let ix = Instruction {
    program,
    accounts: InitSlippyPool {
        slippy: slippy.pubkey(), // if pda: Pubkey::find_program_address(...)
        ...
    },
    data: vec![],
};

// submit transaction with instruction, `slippy` has to sign because
// it's a keypair and you are creating the slippy account with the
// instruction.

let tx = Transaction::new(
    &[ix], // instructions,
    &payer, // payer (keypair)
    &[&payer, &slippy], // signers
)

*/
