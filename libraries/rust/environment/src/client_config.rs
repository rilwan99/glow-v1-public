use std::collections::HashSet;

use glow_margin::Permissions;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};
use thiserror::Error;

use solana_sdk::{program_error::ProgramError, pubkey::Pubkey};

use glow_instructions::test_service::derive_token_mint;
use glow_solana_client::rpc::{ClientError, SolanaRpc, SolanaRpcExtra};

use crate::{
    builder::{resolve_token_mint, BuilderError},
    config::{AirspaceConfig, EnvironmentConfig, OraclePriceConfig, TokenDescription},
};

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("rpc error: {0}")]
    Rpc(#[from] ClientError),

    #[error("builder error: {0}")]
    Builder(#[from] BuilderError),

    #[error("unpack error: {0}")]
    UnpackError(ProgramError),

    #[error("could not read market {0} on the network")]
    MissingMarket(Pubkey),

    #[error("could not read mint {0} on the network")]
    InvalidMint(Pubkey),
}

#[serde_as]
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GlowAppConfig {
    pub tokens: Vec<TokenInfo>,
    pub airspaces: Vec<AirspaceInfo>,
    pub exchanges: Vec<DexInfo>,
}

impl GlowAppConfig {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

impl GlowAppConfig {
    pub async fn from_env_config(
        env: EnvironmentConfig,
        network: &(dyn SolanaRpc + 'static),
        override_lookup_authority: Option<Pubkey>,
    ) -> Result<Self, ConfigError> {
        let mut seen = HashSet::new();
        let mut tokens = vec![];
        let mut airspaces = vec![];

        for airspace in &env.airspaces {
            for token in &airspace.tokens {
                if seen.contains(&token.name) {
                    continue;
                }

                seen.insert(token.name.clone());
                tokens.push(TokenInfo::from_desc(network, token).await?);
            }
            // Override the airspace config address if a default is provided
            let mut airspace = airspace.clone();
            if let Some(override_lookup_authority) = override_lookup_authority {
                airspace.lookup_registry_authority = Some(override_lookup_authority);
            }
            // Sort the token symbols in the airspace
            airspace.tokens.sort_by(|a, b| a.symbol.cmp(&b.symbol));

            airspaces.push(airspace.clone().into());
        }
        // Sort airspaces
        airspaces.sort_by(|a: &AirspaceInfo, b: &AirspaceInfo| a.name.cmp(&b.name));

        let exchanges = env
            .exchanges
            .iter()
            .map(|dex| {
                let base = resolve_token_mint(&env, &dex.base)?;
                let quote = resolve_token_mint(&env, &dex.quote)?;

                let description = dex
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("{}/{}", &dex.base, &dex.quote));

                Ok(DexInfo {
                    description,
                    program: Pubkey::default(),
                    address: Pubkey::default(),
                    base,
                    quote,
                })
            })
            .collect::<Result<_, BuilderError>>()?;

        // Order tokens by symbol
        tokens.sort_by(|a, b| a.symbol.cmp(&b.symbol));

        Ok(Self {
            tokens,
            airspaces,
            exchanges,
        })
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AirspaceInfo {
    pub name: String,
    pub tokens: Vec<String>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub lookup_registry_authority: Option<Pubkey>,

    #[serde_as(as = "Vec<DisplayFromStr>")]
    pub liquidators: Vec<Pubkey>,

    #[serde_as(as = "Vec<DisplayFromStr>")]
    pub refreshers: Vec<Pubkey>,
}

impl From<AirspaceConfig> for AirspaceInfo {
    fn from(config: AirspaceConfig) -> Self {
        Self {
            liquidators: config.cranks_with_permission(Permissions::LIQUIDATE),
            refreshers: config.cranks_with_permission(Permissions::REFRESH_POSITION_CONFIG),
            name: config.name,
            tokens: config.tokens.iter().map(|t| t.name.clone()).collect(),
            lookup_registry_authority: config.lookup_registry_authority,
        }
    }
}

impl Default for AirspaceInfo {
    fn default() -> Self {
        Self {
            name: "default".to_owned(),
            tokens: vec![],
            lookup_registry_authority: None,
            liquidators: vec![],
            refreshers: vec![],
        }
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TokenInfo {
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    pub precision: u8,

    #[serde_as(as = "DisplayFromStr")]
    pub token_program: Pubkey,

    #[serde_as(as = "DisplayFromStr")]
    pub mint: Pubkey,

    pub oracle: OraclePriceConfig,
    pub pyth_feed_id: Option<[u8; 32]>,
    pub pyth_redemption_feed_id: Option<[u8; 32]>,
    pub token_features: u16,
}

impl TokenInfo {
    async fn from_desc(
        network: &(dyn SolanaRpc + 'static),
        desc: &TokenDescription,
    ) -> Result<Self, ConfigError> {
        let mint = desc.mint.unwrap_or_else(|| derive_token_mint(&desc.name));
        let decimals = match desc.decimals {
            Some(d) => d,
            None => network.get_token_mint(&mint).await?.decimals,
        };

        Ok(Self {
            mint,
            oracle: desc.token_oracle.clone(),
            decimals,
            symbol: desc.symbol.clone(),
            name: desc.name.clone(),
            precision: desc.precision,
            token_program: desc.token_program,
            pyth_feed_id: desc
                .pyth_feed_id
                .as_ref()
                .map(|id| glow_program_common::oracle::get_feed_id_from_hex(id)),
            pyth_redemption_feed_id: desc
                .pyth_redemption_feed_id
                .as_ref()
                .map(|id| glow_program_common::oracle::get_feed_id_from_hex(id)),
            token_features: desc.token_features,
        })
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DexInfo {
    pub description: String,

    #[serde_as(as = "DisplayFromStr")]
    pub program: Pubkey,

    #[serde_as(as = "DisplayFromStr")]
    pub address: Pubkey,

    #[serde_as(as = "DisplayFromStr")]
    pub base: Pubkey,

    #[serde_as(as = "DisplayFromStr")]
    pub quote: Pubkey,
}

#[doc(hidden)]
pub mod legacy {
    use indexmap::IndexMap;

    use super::*;
    use std::collections::HashMap;

    pub async fn from_config(config: &super::GlowAppConfig) -> Result<GlowAppConfig, ConfigError> {
        let mut ordered_tokens = config.tokens.clone();
        ordered_tokens.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        let tokens: IndexMap<String, TokenInfo> = ordered_tokens
            .into_iter()
            .map(|t| (t.name.clone(), TokenInfo::from(t)))
            .collect();

        let mut airspaces = vec![];

        for airspace in &config.airspaces {
            airspaces.push(AirspaceInfo {
                name: airspace.name.clone(),
                tokens: airspace.tokens.clone(),
            });
        }
        // Sort airspace tokens
        airspaces.iter_mut().for_each(|airspace| {
            airspace.tokens.sort();
        });
        airspaces.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(GlowAppConfig {
            airspace_program_id: glow_instructions::airspace::AIRSPACE_PROGRAM,
            margin_program_id: glow_instructions::margin::MARGIN_PROGRAM,
            margin_pool_program_id: glow_instructions::margin_pool::MARGIN_POOL_PROGRAM,
            metadata_program_id: glow_metadata::ID,
            faucet_program_id: None,
            url: String::new(),
            tokens,
            airspaces,
            exchanges: HashMap::new(),
        })
    }

    #[serde_as]
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct GlowAppConfig {
        #[serde_as(as = "DisplayFromStr")]
        pub airspace_program_id: Pubkey,

        #[serde_as(as = "DisplayFromStr")]
        pub margin_program_id: Pubkey,

        #[serde_as(as = "DisplayFromStr")]
        pub margin_pool_program_id: Pubkey,

        #[serde_as(as = "DisplayFromStr")]
        pub metadata_program_id: Pubkey,

        #[serde_as(as = "Option<DisplayFromStr>")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub faucet_program_id: Option<Pubkey>,

        pub url: String,

        pub tokens: IndexMap<String, TokenInfo>,
        pub airspaces: Vec<AirspaceInfo>,
        pub exchanges: HashMap<String, ()>,
    }

    #[serde_as]
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AirspaceInfo {
        pub name: String,
        pub tokens: Vec<String>,
    }

    #[serde_as]
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct TokenInfo {
        pub symbol: String,
        pub name: String,
        pub decimals: u8,
        pub precision: u8,

        #[serde_as(as = "Option<DisplayFromStr>")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub faucet: Option<Pubkey>,

        #[serde(skip_serializing_if = "Option::is_none")]
        pub faucet_limit: Option<u64>,

        #[serde_as(as = "DisplayFromStr")]
        pub mint: Pubkey,
    }

    impl From<super::TokenInfo> for TokenInfo {
        fn from(other: super::TokenInfo) -> Self {
            Self {
                symbol: other.symbol,
                name: other.name,
                decimals: other.decimals,
                precision: other.precision,
                mint: other.mint,
                faucet: None,
                faucet_limit: None,
            }
        }
    }
}
