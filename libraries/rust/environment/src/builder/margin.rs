use glow_instructions::margin::derive_token_config;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey, system_program};

use glow_margin::{TokenConfig, TokenConfigUpdate};
use glow_solana_client::rpc::SolanaRpcExtra;
use squads_multisig::anchor_lang::AccountDeserialize;

use super::{Builder, BuilderError, LookupScope};

pub async fn configure_margin_token(
    builder: &mut Builder,
    airspace: &Pubkey,
    mint: &Pubkey,
    name: &str,
    config: Option<TokenConfigUpdate>,
) -> Result<(), BuilderError> {
    // June 2025: we had to migrate to an incompatible account version, and needed to check if the account
    // matched the old layout before configuring it.
    let existing_config = upgrade_token_config(builder, airspace, mint).await?;

    let should_update = match (existing_config, &config) {
        (None, None) => false,
        (Some(existing), Some(update)) => existing != *update,
        _ => true,
    };

    if should_update {
        log::info!("updating margin token config for mint {name} {mint}");

        let ix_builder = builder.margin_config_ix(airspace);
        builder.propose(
            [ix_builder.configure_token(*mint, config.unwrap())],
            Some(format!("configure margin token {name}")),
        )
    }

    builder.register_lookups(
        LookupScope::Airspace,
        [*mint, derive_token_config(airspace, mint)],
    );

    Ok(())
}

// pub async fn get_token_config(
//     builder: &Builder,
//     airspace: &Pubkey,
//     mint: &Pubkey,
// ) -> Result<Option<TokenConfig>, BuilderError> {
//     let address = derive_token_config(airspace, mint);
//     Ok(builder.interface.try_get_anchor_account(&address).await?)
// }

pub async fn upgrade_token_config(
    builder: &mut Builder,
    airspace: &Pubkey,
    mint: &Pubkey,
) -> Result<Option<TokenConfig>, BuilderError> {
    use squads_multisig::anchor_lang::InstructionData;
    use squads_multisig::anchor_lang::ToAccountMetas;

    let address = derive_token_config(airspace, mint);
    let account = builder.interface.get_account(&address).await?;
    let Some(account) = &account else {
        return Ok(None);
    };
    // Check if the account is the old version
    if account.data.len() == 8 + std::mem::size_of::<glow_margin::migrate::TokenConfig>() {
        log::info!("Migrating token config to v2");
        // Get the old account
        let old_config =
            glow_margin::migrate::TokenConfig::try_deserialize(&mut &account.data[..]).unwrap();
        // Migrate it
        let migrate_ix = Instruction {
            program_id: glow_margin::ID,
            accounts: glow_margin::accounts::MigrateTokenConfig {
                authority: builder.proposal_authority(),
                airspace: *airspace,
                payer: builder.payer(),
                mint: *mint,
                token_config: address,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: glow_margin::instruction::MigrateTokenConfig {}.data(),
        };
        builder.propose(
            vec![migrate_ix],
            Some(format!("Migrate token config for mint {mint}")),
        );
        // Return an altered token config to use to determine further changes
        return Ok(Some(TokenConfig {
            mint: old_config.mint,
            mint_token_program: old_config.mint_token_program,
            underlying_mint: old_config.underlying_mint,
            underlying_mint_token_program: old_config.underlying_mint_token_program,
            airspace: old_config.airspace,
            token_kind: old_config.token_kind,
            value_modifier: old_config.value_modifier,
            max_staleness: old_config.max_staleness,
            admin: old_config.admin,
            token_features: Default::default(),
            version: 0,
            reserved: [0; 64],
        }));
    }
    // Fall back
    Ok(builder.interface.try_get_anchor_account(&address).await?)
}
