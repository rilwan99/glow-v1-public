//! A builder that mainly ensures that all required programs are present in the test environment

use std::fs;

const TARGET_DEPLOY_DIR: &str = "../../target/deploy";

fn main() {
    let ecosystem_programs = [
        "jupiter_v6",
        "saber_stable_swap",
        "whirlpool",
        "lookup_table_registry",
        "squads",
        "stake_pool",
        "solayer",
    ];

    // If there's no built anchor program, build it. We check if the deploy folder doesn't exist
    // because the anchor program is built in the deploy folder.
    if fs::metadata(TARGET_DEPLOY_DIR).is_err() {
        // Build the anchor programs
        let _ = std::process::Command::new("anchor")
            .current_dir("../")
            .args(["build", "--", "--features", "testing"])
            .status()
            .unwrap();
    }

    for program in ecosystem_programs.iter() {
        // Check if the binary of the program is present in the target directory
        let program_path = format!("{}/{}.so", TARGET_DEPLOY_DIR, program);
        if fs::metadata(&program_path).is_err() {
            fs::copy(format!("../../deps/{}.so", program), program_path).unwrap();
        }
    }
}
