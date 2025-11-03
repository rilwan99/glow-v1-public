<div align="center">
  <svg width="115" height="100" viewBox="0 0 23 20" fill="none" xmlns="http://www.w3.org/2000/svg">
<path d="M5.97733 5.89909L0 10.7655V20H14.4788L20.2698 15.415C20.4452 15.2764 20.3465 14.9935 20.1231 14.9935H6.96671C6.41995 14.9935 5.97733 14.55 5.97733 14.0021V5.89909Z" fill="#A3A4EC"/>
<path d="M16.3754 5.9073V14.0171L22.2568 9.23583V0H7.78349L2.0857 4.49433C1.9103 4.63302 2.00759 4.91589 2.23095 4.91589H15.386C15.9328 4.91589 16.3754 5.35942 16.3754 5.9073Z" fill="#A3A4EC"/>
</svg>

  <h1>Glow V1</h1>

  <p>
    <a target="_blank" href="https://opensource.org/licenses/AGPL-3.0">
      <img alt="License" src="https://img.shields.io/badge/license-AGPL--3.0--or--later-blue" />
    </a>
  </p>

  <h4>
    <a target="_blank" href="">Website</a>
    |
    <a target="_blank" href="">Docs</a>
  </h4>
</div>

# Glow V1

Glow V1 is a non-custodial borrowing and lending protocol built on Solana. This repository contains the protocol's core implementation and supporting tools, including a web interface.

## Status

The protocol is currently under active development, and all APIs are subject to change.

## Documentation

Auto-generated API docs are available [here](https://blueprint-finance.github.io/glow-v1/ts-client/)

## Getting Started

### Prerequisites

Before running any commands, you'll need to:

1. Install [pnpm](https://pnpm.io/installation), [Solana and Anchor CLI](https://www.anchor-lang.com/docs/installation), [GCloud CLI](https://cloud.google.com/sdk/docs/install) and [Docker](https://docs.docker.com/get-docker/).
2. Have access to the `glow-programs-dev` Google Cloud project
3. Authenticate with Google Artifact Registry (our package repository)
    - Follow the authentication guide in [packages/data-services/README.md](./packages/data-services/README.md#authenticating-to-google-artifact-registry)

To run all the packages in dev mode and start a local validator with the local data services, run:

```bash
pnpm dev:localnet
```

To run just the packages necessary for the frontend, run:

```bach
pnpm dev
```

## Backend

### Local Environment Setup

1. Build programs and move dependencies to `target` directory:

```sh
./scripts/build-anchor-test-programs.sh
```

2. Start a local validator:

```sh
./scripts/run_local_validator.sh -r
```

This will start a Solana validator at `http://localhost:8899`. You can use this URL in Solana Explorer to view accounts and transactions.

> **Note**: If you encounter any issues with the setup, please contact the team.

### IDL Generation

To regenerate IDLs after making program changes:

```sh
./scripts/build-anchor-idls.sh
```

### Test

The main integration tests of various scenarios can be found in `./tests/hosted/tests/`. Note that various linked files are required hence it is not enough to build it with cargo. The way to run all the hosted tests is:

```bash
./check
```

To only run all the integration tests:

```bash
./check cargo-test
```

To run it in a docker container that already contains all the solana and anchor dependencies. This only requires docker:

```bash
./check in-docker
```

Run a single job from the workflow:

```bash
./check [in-docker] [job-name (e.g. e2e-test)]
```

## Turborepo

While it's possible to run the scripts without turbo, running them with turbo will ensure the correct order of execution based on the dependencies. Also, Turbo will catch the packages and prevent them from rebuilding unnecessarily.

Start the development server for the `frontend` and `margin` package:

```bash
pnpm dev
```

Start the development server for the `frontend` and `margin` package, but ALSO start the local validator:

```bash
pnpm dev:localnet
```

Build all packages:

```bash
pnpm build
```

This will build all packages and their dependencies, as defined in the package.json of each package. Turbo will define the order of execution based on the dependencies.

To run a specific command for a package, you can use the following syntax:

```bash
pnpm turbo run [command] --filter=[package]
```

For example, to only build the `frontend` package, run:

```bash
pnpm turbo run build --filter @bpf-glow/frontend
```

Filtering on multiple packages can be done as follows:

```bash
pnpm turbo run build --filter @bpf-glow/frontend --filter @bpf-glow/margin
```

If packages have a dependency on another package (as defined in the package.json of the target package), the dependency will be built first.
