use std::sync::Arc;

use anchor_lang::{InstructionData, ToAccountMetas};
use anyhow::Result;
use glow_instructions::{
    margin::{adapter_invoke, derive_token_config},
    MintInfo,
};
use glow_simulation::solana_rpc_api::SolanaRpcClient;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey, signature::Keypair, signer::Signer};

use crate::send_and_confirm;

pub async fn create_test_service_authority(
    rpc: &Arc<dyn SolanaRpcClient>,
    authority: &Keypair,
) -> Result<()> {
    // Set up the test service authority
    let test_service_authority = derive_test_service_authority();
    let ix = Instruction {
        program_id: glow_test_service::ID,
        accounts: glow_test_service::accounts::InitTestServiceAuthority {
            signer: authority.pubkey(),
            test_service_authority,
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: glow_test_service::instruction::InitTestServiceAuthority.data(),
    };
    send_and_confirm(rpc, &[ix], &[authority]).await?;

    Ok(())
}

pub async fn register_test_adapter_position(
    rpc: &Arc<dyn SolanaRpcClient>,
    signer: &Keypair,
    airspace: Pubkey,
    margin_account: Pubkey,
    position_mint: MintInfo,
) -> Result<Pubkey> {
    let token_config = derive_token_config(&airspace, &position_mint.address);
    let position_account = derive_test_service_position_mint(&position_mint.address);
    let instruction = Instruction {
        program_id: glow_test_service::ID,
        accounts: glow_test_service::accounts::RegisterAdapterPosition {
            owner: signer.pubkey(),
            airspace,
            test_authority: derive_test_service_authority(),
            margin_account,
            token_config,
            position_mint: position_mint.address,
            position_account,
            token_program: position_mint.token_program(),
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: glow_test_service::instruction::RegisterAdapterPosition {}.data(),
    };
    let invoke_ix = adapter_invoke(airspace, signer.pubkey(), margin_account, instruction);
    send_and_confirm(rpc, &[invoke_ix], &[signer]).await?;

    Ok(position_account)
}

pub async fn close_test_adapter_position(
    rpc: &Arc<dyn SolanaRpcClient>,
    signer: &Keypair,
    airspace: Pubkey,
    margin_account: Pubkey,
    position_mint: MintInfo,
) -> Result<Pubkey> {
    let token_config = derive_token_config(&airspace, &position_mint.address);
    let position_account = dbg!(derive_test_service_position_mint(&position_mint.address));
    let instruction = Instruction {
        program_id: glow_test_service::ID,
        accounts: glow_test_service::accounts::CloseAdapterPosition {
            owner: dbg!(signer.pubkey()),
            airspace,
            test_authority: dbg!(derive_test_service_authority()),
            margin_account,
            token_config,
            position_mint: dbg!(position_mint.address),
            position_account,
            token_program: position_mint.token_program(),
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: glow_test_service::instruction::CloseAdapterPosition {}.data(),
    };
    let invoke_ix = adapter_invoke(airspace, signer.pubkey(), margin_account, instruction);
    send_and_confirm(rpc, &[invoke_ix], &[signer]).await?;

    Ok(position_account)
}

pub fn derive_test_service_authority() -> Pubkey {
    Pubkey::find_program_address(
        &[glow_test_service::seeds::TEST_SERVICE_AUTHORITY],
        &glow_test_service::ID,
    )
    .0
}

pub fn derive_test_service_position_mint(position_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            glow_test_service::seeds::TOKEN_ACCOUNT,
            position_mint.as_ref(),
        ],
        &glow_test_service::ID,
    )
    .0
}
