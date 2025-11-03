use glow_instructions::{
    get_metadata_address,
    margin::{derive_token_config, TokenAdmin, TokenConfigUpdate, TokenKind},
    margin_pool::{
        derive_margin_pool, MarginPoolConfiguration, MarginPoolIxBuilder, MARGIN_POOL_PROGRAM,
    },
    MintInfo,
};
use glow_margin::TokenFeatures;
use glow_margin_pool::{MarginPool, TokenMetadataParams};
use glow_metadata::{PositionTokenMetadata, POSITION_TOKEN_METADATA_VERSION};
use glow_program_common::{GOVERNOR_DEVNET, GOVERNOR_MAINNET};
use glow_solana_client::{network::NetworkKind, rpc::SolanaRpcExtra};
use solana_sdk::{instruction::Instruction, system_program};

use super::{Builder, BuilderError, LookupScope, TokenContext};

pub(crate) async fn configure_for_token(
    builder: &mut Builder,
    token: &TokenContext,
) -> Result<(), BuilderError> {
    use squads_multisig::anchor_lang::{AccountSerialize, InstructionData, ToAccountMetas};

    let margin_config_ix = builder.margin_config_ix(&token.airspace);
    let mint_info = MintInfo::with_token_program(token.mint, token.token_program);
    let pool_ix = MarginPoolIxBuilder::new(token.airspace, mint_info);

    let authority = match builder.network {
        NetworkKind::Localnet => builder.signer.pubkey(),
        NetworkKind::Mainnet => GOVERNOR_MAINNET,
        NetworkKind::Devnet => GOVERNOR_DEVNET,
    };
    let payer = match builder.network {
        NetworkKind::Localnet => builder.signer.pubkey(),
        NetworkKind::Mainnet | NetworkKind::Devnet => authority,
    };

    let Some(pool_config) = &token.desc.margin_pool else {
        return Ok(());
    };

    let pool = builder
        .interface
        .try_get_anchor_account::<MarginPool>(&pool_ix.address)
        .await?;

    // Check the pool's token metadata, might also need reconfiguring

    let mut configure_pool_ixns = vec![];

    if pool.is_none() {
        log::info!(
            "create margin pool for token {} at {}",
            &token.desc.name,
            derive_margin_pool(&token.airspace, &token.mint)
        );
        configure_pool_ixns.push(pool_ix.create(authority, payer, None));
    }

    let should_reconfigure = match pool {
        None => true,
        Some(pool) => {
            // Check pool config
            let oracle_changed = pool.token_price_oracle != token.price_oracle;
            if oracle_changed {
                log::info!(
                    "Oracle changed: {:?} to {:?}",
                    pool.token_price_oracle,
                    token.price_oracle
                );
            }
            let config_changed = pool.config != *pool_config;
            if config_changed {
                log::info!("Config changed: {:?} tp {:?}", pool.config, pool_config);
            }
            // Metadata might also be different
            // let old_metadata_size = 8 + std::mem::size_of::<migration::PositionTokenMetadata>();
            let old_metadata_size = 179;
            let deposit_metadata = get_metadata_address(&pool.airspace, &pool.deposit_note_mint);
            let deposit_metadata_account = builder
                .interface
                .get_account(&deposit_metadata)
                .await?
                .expect("An existing pool should have a note metadata");

            // Check the account size, if it's the old size, port it to the new one as the program will migrate it.
            let deposit_metadata = if deposit_metadata_account.data.len() == old_metadata_size {
                let old_metadata = <self::migration::PositionTokenMetadata as squads_multisig::anchor_lang::AnchorDeserialize>::deserialize(&mut &deposit_metadata_account.data[8..]).unwrap();

                let new_metadata = PositionTokenMetadata {
                    airspace: old_metadata.airspace,
                    position_token_mint: old_metadata.position_token_mint,
                    underlying_token_mint: old_metadata.underlying_token_mint,
                    adapter_program: old_metadata.adapter_program,
                    token_program: old_metadata.token_program,
                    token_kind: old_metadata.token_kind,
                    value_modifier: old_metadata.value_modifier,
                    max_staleness: old_metadata.max_staleness,
                    token_features: Default::default(),
                    version: POSITION_TOKEN_METADATA_VERSION,
                    reserved: [0; 64],
                };
                let mut data = vec![];
                new_metadata.try_serialize(&mut data).unwrap();
                let migrate_ix = Instruction {
                    program_id: glow_metadata::ID,
                    accounts: glow_metadata::accounts::SetEntry {
                        payer,
                        authority,
                        airspace: token.airspace,
                        metadata_account: deposit_metadata,
                        system_program: system_program::ID,
                    }
                    .to_account_metas(None),
                    data: glow_metadata::instruction::SetEntry {
                        key_account: pool.deposit_note_mint,
                        offset: 0,
                        data,
                    }
                    .data(),
                };
                builder.propose(
                    vec![migrate_ix],
                    Some(format!(
                        "Migrate deposit metadata for pool {}",
                        pool.address
                    )),
                );
                new_metadata
            } else {
                // It's just easier to fetch the account again, we're going to remove the mess above anyways :)
                // Get the pool's metadata to check if we need to reconfigure params (e.g. max_staleness)

                builder
                    .interface
                    .try_get_anchor_account::<PositionTokenMetadata>(&deposit_metadata)
                    .await?
                    .expect("An existing pool should have a note metadata")
            };

            let loan_metadata = get_metadata_address(&pool.airspace, &pool.loan_note_mint);
            let loan_metadata_account = builder
                .interface
                .get_account(&loan_metadata)
                .await?
                .expect("An existing pool should have a note metadata");

            // Check the account size, if it's the old size, port it to the new one as the program will migrate it.
            let loan_metadata = if loan_metadata_account.data.len() == old_metadata_size {
                let old_metadata = <self::migration::PositionTokenMetadata as squads_multisig::anchor_lang::AnchorDeserialize>::deserialize(&mut &loan_metadata_account.data[8..]).unwrap();

                let new_metadata = PositionTokenMetadata {
                    airspace: old_metadata.airspace,
                    position_token_mint: old_metadata.position_token_mint,
                    underlying_token_mint: old_metadata.underlying_token_mint,
                    adapter_program: old_metadata.adapter_program,
                    token_program: old_metadata.token_program,
                    token_kind: old_metadata.token_kind,
                    value_modifier: old_metadata.value_modifier,
                    max_staleness: old_metadata.max_staleness,
                    token_features: Default::default(),
                    version: POSITION_TOKEN_METADATA_VERSION,
                    reserved: [0; 64],
                };
                let mut data = vec![];
                new_metadata.try_serialize(&mut data).unwrap();
                let migrate_ix = Instruction {
                    program_id: glow_metadata::ID,
                    accounts: glow_metadata::accounts::SetEntry {
                        payer,
                        authority,
                        airspace: token.airspace,
                        metadata_account: loan_metadata,
                        system_program: system_program::ID,
                    }
                    .to_account_metas(None),
                    data: glow_metadata::instruction::SetEntry {
                        key_account: pool.loan_note_mint,
                        offset: 0,
                        data,
                    }
                    .data(),
                };
                builder.propose(
                    vec![migrate_ix],
                    Some(format!("Migrate loan metadata for pool {}", pool.address)),
                );
                new_metadata
            } else {
                // It's just easier to fetch the account again, we're going to remove the mess above anyways :)
                // Get the pool's metadata to check if we need to reconfigure params (e.g. max_staleness)

                builder
                    .interface
                    .try_get_anchor_account::<PositionTokenMetadata>(&loan_metadata)
                    .await?
                    .expect("An existing pool should have a note metadata")
            };

            // We can only legally change the value modifier and max staleness for existing pools
            let max_staleness = deposit_metadata.max_staleness != loan_metadata.max_staleness
                || deposit_metadata.max_staleness != token.desc.max_staleness;
            if max_staleness {
                log::info!(
                    "Margin pool {} max_staleness different, set to {}",
                    pool.address,
                    token.desc.max_staleness
                );
            }
            let collateral_weight = deposit_metadata.value_modifier != token.desc.collateral_weight;
            if collateral_weight {
                log::info!(
                    "Margin pool {} collateral weight different, change: {} > {}",
                    pool.address,
                    deposit_metadata.value_modifier,
                    token.desc.collateral_weight
                );
            }
            let max_leverage = loan_metadata.value_modifier != token.desc.max_leverage;
            if max_leverage {
                log::info!(
                    "Margin pool {} max leverage different, change: {} > {}",
                    pool.address,
                    loan_metadata.value_modifier,
                    token.desc.max_leverage
                );
            }
            let token_features_changed = deposit_metadata.token_features
                != token.desc.token_features
                || loan_metadata.token_features != token.desc.token_features;
            if token_features_changed {
                log::info!(
                    "Margin pool {} token features different, change ({} & {}) > {} ",
                    pool.address,
                    deposit_metadata.token_features,
                    loan_metadata.token_features,
                    token.desc.token_features
                );
            }
            max_staleness
                || collateral_weight
                || max_leverage
                || config_changed
                || oracle_changed
                || token_features_changed
        }
    };

    if should_reconfigure {
        log::info!(
            "configure margin pool for token {} at {}",
            &token.desc.name,
            derive_margin_pool(&token.airspace, &token.mint)
        );

        configure_pool_ixns.push(pool_ix.configure(
            authority,
            payer,
            &MarginPoolConfiguration {
                metadata: Some(TokenMetadataParams {
                    token_kind: glow_metadata::TokenKind::Collateral,
                    collateral_weight: token.desc.collateral_weight,
                    max_leverage: token.desc.max_leverage,
                    max_staleness: token.desc.max_staleness,
                    token_features: token.desc.token_features,
                }),
                parameters: Some(*pool_config),
                token_oracle: Some(token.price_oracle),
            },
        ));
    }

    // marker

    let note_configs = builder
        .upgrade_margin_token_configs(
            &token.airspace,
            &[pool_ix.deposit_note_mint, pool_ix.loan_note_mint],
        )
        .await?;

    let should_update_deposit = note_configs[0]
        .as_ref()
        .map(|c| {
            c.value_modifier != token.desc.collateral_weight
                || c.max_staleness != token.desc.max_staleness
                || c.token_features.bits() != token.desc.token_features
        })
        .unwrap_or(true);
    let should_update_loan = note_configs[1]
        .as_ref()
        .map(|c| {
            c.value_modifier != token.desc.max_leverage
                || c.max_staleness != token.desc.max_staleness
                || c.token_features.bits() != token.desc.token_features
        })
        .unwrap_or(true);

    if !configure_pool_ixns.is_empty() {
        builder.propose(
            configure_pool_ixns,
            Some(format!("configure pool for token {}", token.desc.name)),
        );
    }

    let mut pool_note_update_ix = vec![];

    let token_features =
        TokenFeatures::from_bits(token.desc.token_features).expect("Invalid token features");

    if should_update_deposit {
        pool_note_update_ix.push(margin_config_ix.configure_token(
            pool_ix.deposit_note_mint,
            TokenConfigUpdate {
                underlying_mint: token.mint,
                underlying_mint_token_program: token.token_program,
                admin: TokenAdmin::Adapter(MARGIN_POOL_PROGRAM),
                token_kind: TokenKind::Collateral,
                value_modifier: token.desc.collateral_weight,
                max_staleness: token.desc.max_staleness,
                token_features,
            },
        ));
    }

    if should_update_loan {
        pool_note_update_ix.push(margin_config_ix.configure_token(
            pool_ix.loan_note_mint,
            TokenConfigUpdate {
                underlying_mint: token.mint,
                underlying_mint_token_program: token.token_program,
                admin: TokenAdmin::Adapter(MARGIN_POOL_PROGRAM),
                token_kind: TokenKind::Claim,
                value_modifier: token.desc.max_leverage,
                max_staleness: token.desc.max_staleness,
                token_features,
            },
        ));
    }

    if !pool_note_update_ix.is_empty() {
        builder.propose(
            pool_note_update_ix,
            Some(format!(
                "configure pool deposit and loan {}",
                token.desc.name
            )),
        );
    }

    builder.register_lookups(
        LookupScope::Pools,
        [
            pool_ix.address,
            pool_ix.vault,
            pool_ix.deposit_note_mint,
            pool_ix.loan_note_mint,
            derive_token_config(&token.airspace, &pool_ix.deposit_note_mint),
            derive_token_config(&token.airspace, &pool_ix.loan_note_mint),
        ],
    );

    Ok(())
}

mod migration {

    use squads_multisig::anchor_lang::prelude::*;
    #[derive(AnchorSerialize, AnchorDeserialize, Debug, Eq, PartialEq)]
    pub struct PositionTokenMetadata {
        /// The airspace that the entry belongs to
        pub airspace: solana_sdk::pubkey::Pubkey,

        /// The mint for the position token
        pub position_token_mint: solana_sdk::pubkey::Pubkey,

        /// The underlying token represented by this position
        pub underlying_token_mint: solana_sdk::pubkey::Pubkey,

        /// The adapter program in control of this position
        pub adapter_program: solana_sdk::pubkey::Pubkey,

        /// The token program of this position
        pub token_program: solana_sdk::pubkey::Pubkey,

        /// Description of this token
        pub token_kind: glow_metadata::TokenKind,

        /// A modifier to adjust the token value, based on the kind of token
        pub value_modifier: u16,

        /// The maximum staleness (seconds) that's acceptable for balances of this token
        pub max_staleness: u64,
    }
}
