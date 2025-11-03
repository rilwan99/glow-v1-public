use std::path::{Path, PathBuf};

use glow_margin::Permissions;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};
use thiserror::Error;

use solana_sdk::pubkey::Pubkey;

use glow_margin_pool::MarginPoolConfig;
use glow_solana_client::network::NetworkKind;

pub static DEFAULT_MARGIN_ADAPTERS: &[Pubkey] =
    &[glow_instructions::margin_pool::MARGIN_POOL_PROGRAM];

/// Description of errors that occur when reading configuration
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("failed while trying I/O on {path}: {error}")]
    IoError {
        path: PathBuf,
        error: std::io::Error,
    },

    #[error("failed while parsing toml in {path}: {error}")]
    Toml {
        path: PathBuf,
        error: toml::de::Error,
    },

    #[error("missing config directory for airspace {0}")]
    MissingAirspaceDir(PathBuf),
}
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentConfig {
    /// The network this environment should exist within
    pub network: NetworkKind,

    /// List of programs that are allowed to be adapters in the margin system
    pub margin_adapters: Vec<Pubkey>,

    /// The authority allowed to adjust oracle prices in test environments
    pub oracle_authority: Option<Pubkey>,

    /// The airspaces that should exist for this environment
    pub airspaces: Vec<AirspaceConfig>,

    /// The DEX markets available to the environment
    pub exchanges: Vec<DexConfig>,
}

/// Describe an airspace to initialize
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AirspaceConfig {
    /// The name for the airspace
    pub name: String,

    /// If true, user registration with the airspace is restricted by an authority
    pub is_restricted: bool,

    /// The list of addresses authorized to act as cranks in the airspace
    pub cranks: Vec<CrankWithPermissions>,

    /// The tokens to be configured for use in the airspace
    pub tokens: Vec<TokenDescription>,

    /// The lookup registry authority
    pub lookup_registry_authority: Option<Pubkey>,

    /// The adapters that are registered on the airspace
    pub margin_adapters: Vec<Pubkey>,
}

impl AirspaceConfig {
    pub fn cranks_with_permission(&self, permission: Permissions) -> Vec<Pubkey> {
        self.cranks
            .iter()
            .filter_map(|crank| {
                let permissions = Permissions::from_bits(crank.permissions)?;
                if permissions.contains(permission) {
                    return Some(crank.address);
                }
                None
            })
            .collect()
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CrankWithPermissions {
    #[serde_as(as = "DisplayFromStr")]
    pub address: Pubkey,
    pub permissions: u32,
}

/// A description for a token to be created
#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TokenDescription {
    /// The symbol for the token
    pub symbol: String,

    /// The name for the token
    pub name: String,

    /// The number of decimals the token should have
    #[serde(default)]
    pub decimals: Option<u8>,

    /// The decimal precision when displaying token values
    pub precision: u8,

    /// The mint for the token (if it already exists)
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub mint: Option<Pubkey>,

    /// The token program of the mint
    #[serde_as(as = "DisplayFromStr")]
    pub token_program: Pubkey,

    /// The token oracle for the token
    #[serde(default)]
    pub token_oracle: OraclePriceConfig,

    /// The pyth feed ID if the token oracle is a Pyth variant
    pub pyth_feed_id: Option<String>,

    /// The redemption pyth feed ID if the token oracle is a Pyth redemption variant
    pub pyth_redemption_feed_id: Option<String>,

    /// The maximum amount a user can request for an airdrop (when using test tokens)
    #[serde(default)]
    pub max_test_amount: Option<u64>,

    /// The adjustment of value for this token when used as collateral.
    pub collateral_weight: u16,

    /// The maximum leverage allowed for loans of this token.
    pub max_leverage: u16,

    /// The maximum number of seconds since the last update before the oracle/token is
    /// considered stale
    pub max_staleness: u64,

    /// The configuration to use for this token's margin pool (if it should exist)
    #[serde(default)]
    pub margin_pool: Option<MarginPoolConfig>,

    pub token_features: u16,
}

#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
pub enum OraclePriceConfig {
    #[default]
    NoOracle,
    PythPull,
    PythPullRedemption,
}

/// Information about a DEX available to an environment
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
pub struct DexConfig {
    /// The program that does token exchanging
    pub program: String,

    /// A description for this DEX market/pool
    #[serde(default)]
    pub description: Option<String>,

    /// The address of the primary state account used for the token exchange
    #[serde(default)]
    pub state: Option<Pubkey>,

    /// The primary token in the pair that can be exchanged
    pub base: String,

    /// THe secondary token in the pair that can be exchanged
    pub quote: String,
}

#[serde_as]
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
pub struct VaultOracleConfig {
    #[serde(default)]
    pub oracle: OraclePriceConfig,

    #[serde(default)]
    pub pyth_feed_id: Option<String>,

    #[serde(default)]
    pub pyth_redemption_feed_id: Option<String>,
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VaultConfig {
    /// The airspace seed that the vault belongs to
    pub airspace: String,

    /// The configured token name that backs the vault
    pub underlying_token: String,

    /// Optional explicit underlying mint. If omitted, the token definition is used.
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying_mint: Option<Pubkey>,

    /// Optional explicit underlying mint token program. Defaults to the token definition.
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying_mint_token_program: Option<Pubkey>,

    /// The index used to derive the vault PDA for the token / airspace pair
    pub vault_index: u8,

    /// Optional override for the vault authority. Defaults to the proposal authority.
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_authority: Option<Pubkey>,

    /// Whether user deposits are enabled
    #[serde(default)]
    pub enable_deposits: bool,

    /// Whether user withdrawals are enabled
    #[serde(default)]
    pub enable_withdrawals: bool,

    /// Whether operator accounts are enabled
    #[serde(default)]
    pub enable_operators: bool,

    /// Deposit limit for the vault
    #[serde(default)]
    pub deposit_limit: Option<u64>,

    /// Withdrawal limit for the vault
    #[serde(default)]
    pub withdrawal_limit: Option<u64>,

    /// Waiting period prior to executing withdrawals
    #[serde(default)]
    pub withdrawal_waiting_period: Option<i64>,

    /// Minimum deposit size allowed
    #[serde(default)]
    pub minimum_deposit: Option<u64>,

    /// Threshold at which the share exchange rate resets
    #[serde(default)]
    pub minimum_shares_dust_threshold: Option<u64>,

    /// Performance fee, expressed in basis points
    #[serde(default)]
    pub performance_fee_bps: Option<u16>,

    /// Management fee, expressed in basis points
    #[serde(default)]
    pub management_fee_bps: Option<u16>,

    /// Optional display name for the vault (max 27 bytes)
    #[serde(default)]
    pub vault_name: Option<String>,

    /// Optional oracle configuration. Defaults to the underlying token oracle.
    #[serde(default)]
    pub oracle: Option<VaultOracleConfig>,

    /// Optional override for the pyth program used to derive oracle accounts
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pyth_program: Option<Pubkey>,
}

#[serde_as]
#[derive(Serialize, Deserialize)]
struct EnvRootAirspaceConfig {
    name: String,

    #[serde(default)]
    is_restricted: bool,

    #[serde(default)]
    cranks: Vec<CrankWithPermissions>,
    #[serde_as(as = "Option<DisplayFromStr>")]
    lookup_registry_authority: Option<Pubkey>,

    #[serde_as(as = "Vec<DisplayFromStr>")]
    margin_adapters: Vec<Pubkey>,
}

#[serde_as]
#[derive(Serialize, Deserialize)]
struct EnvRootConfigFile {
    network: NetworkKind,
    airspace: Vec<EnvRootAirspaceConfig>,

    #[serde_as(as = "Vec<DisplayFromStr>")]
    #[serde(default)]
    margin_adapters: Vec<Pubkey>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    oracle_authority: Option<Pubkey>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    lookup_registry_authority: Option<Pubkey>,
}

pub fn read_env_config_dir(path: &Path) -> Result<EnvironmentConfig, ConfigError> {
    let root_file = path.join("env.toml");
    let dex_file = path.join("exchanges.toml");

    let root_content =
        std::fs::read_to_string(&root_file).map_err(|error| ConfigError::IoError {
            path: root_file.clone(),
            error,
        })?;
    let root =
        toml::from_str::<EnvRootConfigFile>(&root_content).map_err(|error| ConfigError::Toml {
            path: root_file.clone(),
            error,
        })?;

    let exchanges = read_dex_config_file(&dex_file)?;
    let airspaces = root
        .airspace
        .into_iter()
        .map(|config| {
            let airspace_config_path = path.join(&config.name);

            if !airspace_config_path.exists() || !airspace_config_path.is_dir() {
                return Err(ConfigError::MissingAirspaceDir(airspace_config_path));
            }

            read_airspace_dir(config, &airspace_config_path)
        })
        .collect::<Result<_, _>>()?;

    let margin_adapters = match root.margin_adapters.len() {
        0 => DEFAULT_MARGIN_ADAPTERS.to_vec(),
        _ => root.margin_adapters,
    };

    Ok(EnvironmentConfig {
        network: root.network,
        oracle_authority: root.oracle_authority,
        margin_adapters,
        airspaces,
        exchanges,
    })
}

fn read_airspace_dir(
    config: EnvRootAirspaceConfig,
    path: &Path,
) -> Result<AirspaceConfig, ConfigError> {
    let files = std::fs::read_dir(path)
        .map_err(|error| ConfigError::IoError {
            path: path.to_path_buf(),
            error,
        })?
        .filter_map(|entry| match entry {
            Err(error) => Some(Err(ConfigError::IoError {
                path: path.to_path_buf(),
                error,
            })),
            Ok(entry) if entry.path().extension().unwrap_or_default() == "toml" => {
                Some(Ok(entry.path()))
            }
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?;

    let tokens = files
        .into_iter()
        .map(|f| read_token_desc_from_file(&f))
        .collect::<Result<_, _>>()?;

    Ok(AirspaceConfig {
        tokens,
        name: config.name,
        cranks: config.cranks,
        margin_adapters: config.margin_adapters,
        is_restricted: config.is_restricted,
        lookup_registry_authority: config.lookup_registry_authority,
    })
}

fn read_dex_config_file(path: &Path) -> Result<Vec<DexConfig>, ConfigError> {
    #[derive(Serialize, Deserialize)]
    struct DexConfigFile {
        dex: Vec<DexConfig>,
    }

    if !path.exists() {
        return Ok(vec![]);
    }

    let file_content = std::fs::read_to_string(path).map_err(|error| ConfigError::IoError {
        path: path.to_path_buf(),
        error,
    })?;
    let desc =
        toml::from_str::<DexConfigFile>(&file_content).map_err(|error| ConfigError::Toml {
            path: path.to_path_buf(),
            error,
        })?;

    Ok(desc.dex)
}

fn read_token_desc_from_file(path: &Path) -> Result<TokenDescription, ConfigError> {
    #[derive(Serialize, Deserialize)]
    struct FileTokenDesc {
        token: TokenDescription,
        #[serde(default)]
        margin_pool: Option<MarginPoolConfig>,
    }

    let file_content = std::fs::read_to_string(path).map_err(|error| ConfigError::IoError {
        path: path.to_path_buf(),
        error,
    })?;
    let desc =
        toml::from_str::<FileTokenDesc>(&file_content).map_err(|error| ConfigError::Toml {
            path: path.to_path_buf(),
            error,
        })?;

    Ok(TokenDescription {
        margin_pool: desc.margin_pool,
        ..desc.token
    })
}
