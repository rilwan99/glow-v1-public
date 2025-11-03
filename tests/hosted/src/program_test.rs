use glow_simulation::{anchor_processor, runtime::SolanaProgram};
use solana_program::pubkey;
use solana_sdk::pubkey::Pubkey;
use squads_multisig::squads_multisig_program;

pub const JUP_V6: Pubkey = pubkey!("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4");
pub const SOLAYER: Pubkey = pubkey!("sSo1iU21jBrU9VaJ8PJib1MtorefUV4fzC9GURa2KNn");

/// Create test context.
///
/// If you don't provide a name, gets the name of the current function name and
/// uses it to create a test context. Only use this way when called directly in
/// the test function. If you want to call this in a helper function, pass a
/// name that is unique to the individual test.
#[macro_export]
macro_rules! solana_test_context {
    () => {
        $crate::program_test::TestRuntimeRpcClient::new(&$crate::fn_name_and_try_num!()).await
    };
    ($name:expr) => {
        $crate::program_test::TestRuntimeRpcClient::new($name).await
    };
}

pub fn __type_name_of<T>(_: T) -> &'static str {
    std::any::type_name::<T>()
}

pub fn current_test_attempt_number() -> String {
    std::env::var("__NEXTEST_ATTEMPT").unwrap_or("1".to_string())
}

/// Returns a string with the fully qualified name of the current function,
/// followed by the nextest attempt number (increments on retry).
/// Example: "liquidate::can_withdraw_some_during_liquidation-try_1"
#[macro_export]
macro_rules! fn_name_and_try_num {
    () => {
        format!(
            "{}-try_{}",
            $crate::program_test::__type_name_of(|| {}).replace("::{{closure}}", ""),
            $crate::program_test::current_test_attempt_number()
        )
    };
}

pub fn get_programs() -> Vec<SolanaProgram> {
    std::env::set_var("SBF_OUT_DIR", "../../target/deploy");
    vec![
        SolanaProgram {
            program_id: glow_test_service::ID,
            program_name: "glow_test_service".into(),
            builtin_function: anchor_processor!(glow_test_service),
        },
        SolanaProgram {
            program_id: glow_margin::ID,
            program_name: "glow_margin".into(),
            builtin_function: anchor_processor!(glow_margin),
        },
        SolanaProgram {
            program_id: glow_metadata::ID,
            program_name: "glow_metadata".into(),
            builtin_function: anchor_processor!(glow_metadata),
        },
        SolanaProgram {
            program_id: glow_airspace::ID,
            program_name: "glow_airspace".into(),
            builtin_function: anchor_processor!(glow_airspace),
        },
        SolanaProgram {
            program_id: glow_margin_pool::ID,
            program_name: "glow_margin_pool".into(),
            builtin_function: anchor_processor!(glow_margin_pool),
        },
        SolanaProgram {
            program_id: lookup_table_registry::ID,
            program_name: "lookup_table_registry".into(),
            builtin_function: anchor_processor!(lookup_table_registry),
        },
        SolanaProgram {
            program_id: JUP_V6,
            program_name: "jupiter_v6".into(),
            builtin_function: None,
        },
        SolanaProgram {
            program_id: whirlpool::ID,
            program_name: "whirlpool".into(),
            builtin_function: anchor_processor!(whirlpool),
        },
        SolanaProgram {
            program_id: stable_swap_client::ID,
            program_name: "saber_stable_swap".into(),
            builtin_function: None,
        },
        SolanaProgram {
            program_id: squads_multisig_program::ID,
            program_name: "squads".into(),
            builtin_function: anchor_processor!(squads_multisig_program),
        },
        SolanaProgram {
            program_id: SOLAYER,
            program_name: "solayer".into(),
            builtin_function: None,
        },
        SolanaProgram {
            program_id: spl_stake_pool::ID,
            program_name: "stake_pool".into(),
            builtin_function: None,
        },
    ]
}
