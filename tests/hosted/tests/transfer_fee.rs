#[cfg(test)]
mod tests {
    use anchor_lang::prelude::*;

    use anchor_spl::token_2022::spl_token_2022::{
        extension::{
            transfer_fee::instruction::initialize_transfer_fee_config, ExtensionType,
            StateWithExtensions,
        },
        instruction::initialize_mint,
        state::Mint as Mint2022,
    };
    use glow_margin_pool::{util::validate_zero_transfer_fee, ErrorCode};
    use solana_program_test::ProgramTest;
    use solana_sdk::{
        instruction::Instruction,
        pubkey::Pubkey,
        rent::Rent,
        signature::Keypair,
        signer::{SeedDerivable, Signer},
        system_instruction::create_account,
        transaction::{Result, Transaction},
    };

    fn build_batch_instructions(
        payer_pubkey: &Pubkey,
        mint_pubkey: &Pubkey,
        mint_auth_pubkey: &Pubkey,
        freeze_authority_pubkey: &Pubkey,
        transfer_fee_basis_points: u16,
        maximum_fee: u64,
    ) -> Result<Vec<Instruction>> {
        const TOKEN_PROGRAM: Pubkey = anchor_spl::token_2022::ID;

        // Mint instruction
        let init_mint_ix = initialize_mint(
            &TOKEN_PROGRAM,
            mint_pubkey,
            mint_auth_pubkey,
            Some(freeze_authority_pubkey),
            6,
        )
        .unwrap();

        // TransferFeeConfig instruction
        let transfer_fee_config_ix = initialize_transfer_fee_config(
            &TOKEN_PROGRAM,
            mint_pubkey,
            Some(mint_auth_pubkey),
            Some(&Pubkey::new_unique()), // Withdraw withheld authority
            transfer_fee_basis_points,
            maximum_fee,
        )
        .unwrap();

        // Fund account
        let rent = Rent::default();
        let mint_space = ExtensionType::try_calculate_account_len::<Mint2022>(&[
            ExtensionType::TransferFeeConfig,
        ])
        .unwrap();

        let mint_lamports = rent.minimum_balance(mint_space);
        let create_mint_ix = create_account(
            payer_pubkey,
            mint_pubkey,
            mint_lamports,
            mint_space as u64,
            &TOKEN_PROGRAM,
        );

        Ok(vec![create_mint_ix, transfer_fee_config_ix, init_mint_ix])
    }

    // Create deterministic keypairs for debugging purposes
    fn keypair_from_byte_seed(byte: u8) -> Keypair {
        let seed = [byte; 32];
        Keypair::from_seed(&seed).expect("Invalid keypair bytes")
    }

    #[tokio::test]
    async fn test_validate_zero_transfer_fee_ok() {
        let program_test = ProgramTest::default();
        let (mut client, payer, last_blockhash) = program_test.start().await;

        // Setup keys
        let mint_keys = keypair_from_byte_seed(1);
        let mint_pubkey = mint_keys.pubkey();
        let mint_auth = keypair_from_byte_seed(2);
        let freeze_auth = keypair_from_byte_seed(3);
        let freeze_pubkey = freeze_auth.pubkey();
        let freeze_authority_pubkey = Some(&freeze_pubkey);

        let ixs = build_batch_instructions(
            &payer.pubkey(),
            &mint_pubkey,
            &mint_auth.pubkey(),
            freeze_authority_pubkey.unwrap(),
            0, // NO TRANSFER FEE
            0, // NO MAX FEE
        )
        .unwrap();

        let mut tx = Transaction::new_with_payer(&ixs, Some(&payer.pubkey()));
        tx.sign(&[&payer, &mint_keys], last_blockhash);
        client.process_transaction(tx).await.unwrap();

        let mut mint_account = client.get_account(mint_pubkey).await.unwrap().unwrap();

        let mut mint_data = mint_account.data.clone();
        let mint_info = AccountInfo::new(
            &mint_pubkey,
            false,
            false,
            &mut mint_account.lamports,
            &mut mint_data,
            &anchor_spl::token_2022::ID,
            false,
            0,
        );

        let data = mint_info.try_borrow_data().unwrap();
        let state = StateWithExtensions::<Mint2022>::unpack(&data).unwrap();
        let result = validate_zero_transfer_fee(state);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_non_zero_transfer_fee_error() {
        let program_test = ProgramTest::default();
        let (mut client, payer, last_blockhash) = program_test.start().await;

        // Setup mint account
        let mint_keys = keypair_from_byte_seed(1);
        let mint_pubkey = mint_keys.pubkey();
        let mint_auth = keypair_from_byte_seed(2);
        let freeze_auth = keypair_from_byte_seed(3);
        let freeze_pubkey = freeze_auth.pubkey();
        let freeze_authority_pubkey = Some(&freeze_pubkey);

        let ixs = build_batch_instructions(
            &payer.pubkey(),
            &mint_pubkey,
            &mint_auth.pubkey(),
            freeze_authority_pubkey.unwrap(),
            50, // fee enabled
            1000,
        )
        .unwrap();

        let mut tx = Transaction::new_with_payer(&ixs, Some(&payer.pubkey()));
        tx.sign(&[&payer, &mint_keys], last_blockhash);
        client.process_transaction(tx).await.unwrap();

        let mut mint_account = client.get_account(mint_pubkey).await.unwrap().unwrap();

        let mut mint_data = mint_account.data.clone();
        let mint_info = AccountInfo::new(
            &mint_pubkey,
            false,
            false,
            &mut mint_account.lamports,
            &mut mint_data,
            &anchor_spl::token_2022::ID,
            false,
            0,
        );

        let data = mint_info.try_borrow_data().unwrap();
        let state = StateWithExtensions::<Mint2022>::unpack(&data).unwrap();
        let result = validate_zero_transfer_fee(state);
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap(),
            ErrorCode::TokenExtensionNotEnabled.into()
        );
    }
}
