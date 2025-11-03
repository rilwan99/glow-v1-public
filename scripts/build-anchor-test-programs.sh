#!/bin/bash

set -euxo pipefail

# Set the script to run from the root of the project, and not this or any other directory
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/.."

# Rust version via rustup, set to blank to use the default
RUST_VERSION="rustup run nightly-2025-04-09"

# Change to the project root directory
cd "$PROJECT_ROOT"

# Build the programs with test features enabled
# anchor build --no-docs -- --features testing

# Build the programs with test features enabled using nightly, when we switch anchor to 0.31.0 or a later version that addresses the proc-macro2 version issue, then we can revert the build with the stable rust version
${RUST_VERSION} anchor build --no-docs -- --features testing

# Build the tools that we need to start the local validator
${RUST_VERSION} cargo build

