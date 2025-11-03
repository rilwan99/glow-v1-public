#!/bin/bash

set -e

if [[ ${SOLANA_LOGS:-false} == true ]]; then
	solana -ul logs &
fi

solana-keygen new --no-bip39-passphrase -o lookup-authority.json --force
solana airdrop -k lookup-authority.json -ul 5
cargo run --bin glowctl -- apply -ul --no-confirm config/localnet/
cargo run --bin glowctl -- generate-app-config -ul --no-confirm config/localnet/ -o localnet.config.json --override-lookup-authority $(solana-keygen pubkey lookup-authority.json)
cargo run --bin glow-alt-registry-client -- create-registry -ul --no-confirm -a lookup-authority.json -k lookup-authority.json
cargo run --bin glow-alt-registry-client -- update-registry -ul --no-confirm -a lookup-authority.json -k lookup-authority.json --airspace-name default
cargo run --bin glow-oracle-mirror -- -s ${SOLANA_MAINNET_RPC:='https://api.mainnet-beta.solana.com'} -tl &

echo "waiting for oracles ..."

	while true; do
		if [[ -f tests/oracle-mirror.pid ]]; then
			break;
		fi
		sleep 5
	done
	echo "oracles ready!"

sleep 5
yarn build --force
sleep 5

