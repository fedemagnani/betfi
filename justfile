# Schedina task runner. Three Cargo builds, each pinned by its own rust-toolchain.toml:
#   ./ (crates/)  pascal-rs, pricing, pascal-mock, desk on current stable (root Cargo.toml)
#   programs/     Anchor 1.2.2, host Rust 1.97.1 (SBF build via the installed Agave CLI)
#   app/          Dioxus 0.7 web app, wasm32-unknown-unknown, standalone crate on stable

set dotenv-load := false

default:
    @just --list

# fmt --check + clippy -D warnings + tests in the crates/ and programs/ workspaces, then the app
check: crates-check program-check app-check

# --- crates/ (root workspace) -------------------------------------------

crates-fmt:
    cargo fmt --all --check

crates-clippy:
    cargo clippy --workspace --all-targets --locked -- -D warnings

crates-test:
    cargo test --workspace --locked

crates-check: crates-fmt crates-clippy crates-test

# Pascal mock (tape replay + matcher, steps 1.D/2.D); args go to the binary
mock *args:
    cargo run --locked -p pascal-mock -- {{args}}

# The demo night at 60x: leg 1 (PHI) resolves 40 s after boot (tapes/README.md, "Demo")
mock-demo *args:
    cargo run --locked -p pascal-mock -- --tape tapes/nba-2026-10-28 --speed 60 {{args}}

# Desk: quoter, hedge desk, crank, indexer (steps 1.E, 2.A-2.C); args go to the binary
desk *args:
    cargo run --locked -p desk -- {{args}}

# Builds the `--features devnet` .so (it carries the mock-USDC faucet; SBPF v0 like devnet,
# D-006/D-019) into programs/target/deploy-devnet/, then runs the #[ignore]d chain tests, which
# start their own solana-test-validator on private ports with SIMD-0500 deactivated and generate
# every key in a temp dir.
# Crank + indexer on localnet with the Pascal mock (step 2.C, l2-contracts L2-6)
desk-chain-test:
    cd programs && cargo build-sbf --manifest-path schedina/Cargo.toml --features devnet --sbf-out-dir target/deploy-devnet
    rm -f programs/target/deploy-devnet/schedina-keypair.json
    cargo test --locked -p desk --test chain -- --ignored --nocapture
    cargo test --locked -p desk --test tipsters_chain -- --ignored --nocapture

# Step 4.D localnet proof (L4-3): schedina (devnet features) and mock-amm as SBPF v0 .so files on a
# private solana-test-validator; a mockBONK pool, the desk's /tokens and /swap-quote, and one v0
# transaction [swap, Ed25519 verify, open_pending] that escrows min_out_u; a failed swap leaves no
# Pending. Every key is generated in a temp dir; the validator is killed at the end.
swap-chain-test:
    cd programs && cargo build-sbf --manifest-path schedina/Cargo.toml --features devnet --sbf-out-dir target/deploy-devnet
    cd programs && cargo build-sbf --manifest-path mock-amm/Cargo.toml --sbf-out-dir target/deploy-devnet
    rm -f programs/target/deploy-devnet/schedina-keypair.json programs/target/deploy-devnet/mock_amm-keypair.json
    cargo test --locked -p desk --test swap_chain -- --ignored --nocapture

# Step 8.B localnet proof (L8): the desk's sponsored-transaction relay on a private
# solana-test-validator with the devnet-feature .so. A user with 0 SOL claims test USDC and bets
# through POST /relay (the sponsor pays fees and rent); a second claim within the hour and a System
# transfer out of the sponsor are refused. Keys in a temp dir; the validator is killed at the end.
relay-chain-test:
    cd programs && cargo build-sbf --manifest-path schedina/Cargo.toml --features devnet --sbf-out-dir target/deploy-devnet
    rm -f programs/target/deploy-devnet/schedina-keypair.json
    cargo test --locked -p desk --test relay_chain -- --ignored --nocapture

# Step 7.0c localnet proof (L7-1): `crank amm-peg` on a private solana-test-validator with both
# SBPF v0 .so files (as swap-chain-test): a dry run sends nothing, --yes moves the mockSOL pool within
# 1% of the target (sell and buy), a re-run sends nothing, a non-admin is refused, --from-fx walks a
# large move in steps of at most 10% of the reserves. Keys in a temp dir; the validator is killed.
amm-peg-test:
    cd programs && cargo build-sbf --manifest-path schedina/Cargo.toml --features devnet --sbf-out-dir target/deploy-devnet
    cd programs && cargo build-sbf --manifest-path mock-amm/Cargo.toml --sbf-out-dir target/deploy-devnet
    rm -f programs/target/deploy-devnet/schedina-keypair.json programs/target/deploy-devnet/mock_amm-keypair.json
    cargo test --locked -p desk --test amm_peg_chain -- --ignored --nocapture

# (step 3.D', mode M; docs/live.md) The desk signs a 3-leg hedge batch with throwaway keys against
# the in-process mock; prints and verifies it, never sends to Pascal. Optional arg: a dir to keep it
# Signed-not-sent evidence: one signed hedge batch, verified, never sent [OUT_DIR]
sign-demo *args:
    scripts/sign_demo.sh {{args}}

# Plan 3.A: the whole lifecycle on localnet with the Pascal mock (dev profile): validator, crank
# init/faucet/fund, mock at 60x with permit verification, the desk, a scripted user (two tickets:
# cash-out and settle), vault and hedge checkpoints, the ticket timeline. Kills everything it
# starts. E2E_KEEP=1 keeps the temp dir with the logs. See docs/runbook.md.
e2e:
    cd programs && cargo build-sbf --manifest-path schedina/Cargo.toml --features devnet --sbf-out-dir target/deploy-devnet
    rm -f programs/target/deploy-devnet/schedina-keypair.json
    cargo build --locked -p desk --bins -p pascal-mock
    E2E_SKIP_BUILD=1 scripts/e2e_lifecycle.sh

# --- programs/ ----------------------------------------------------------

# SBF build into programs/target/deploy/schedina.so. anchor build checks declare_id! against
# target/deploy/schedina-keypair.json: copy the real key there when this checkout has it (main
# checkout), else build with --ignore-keys (clean clones, CI). Never run `anchor keys sync`.
program-build:
    #!/usr/bin/env bash
    set -euo pipefail
    cd programs
    if [ -f ../keys/schedina-program-keypair.json ]; then
      mkdir -p target/deploy && install -m 600 ../keys/schedina-program-keypair.json target/deploy/schedina-keypair.json
      # 4.D: the mock AMM's program key, same rule (keys/mock-amm-program-keypair.json).
      if [ -f ../keys/mock-amm-program-keypair.json ]; then install -m 600 ../keys/mock-amm-program-keypair.json target/deploy/mock_amm-keypair.json; fi
      anchor build
    else
      anchor build --ignore-keys
    fi

program-fmt:
    cd programs && cargo fmt --all --check

program-clippy:
    cd programs && cargo clippy --workspace --all-targets --locked -- -D warnings

# LiteSVM tests load target/deploy/schedina.so, so build first
program-test: program-build
    cd programs && cargo test --workspace --locked

program-check: program-fmt program-clippy program-test

# --- app/ ---------------------------------------------------------------

# Needs `rustup target add wasm32-unknown-unknown`. The app crate only builds for wasm32; its
# transaction builders (app/tx, no web deps) are tested on the host.
app-check:
    cd app && cargo fmt --all --check
    cd app && cargo clippy --locked --target wasm32-unknown-unknown -- -D warnings
    cd app && cargo test --locked -p schedina-tx
    cd app && node scripts/slate_check.mjs
    cd app && node scripts/design_lint.mjs
    # The BetFi profile message (6.1e, L6-2) byte for byte; the module has no dependencies.
    cd app && mkdir -p target && rustc --edition 2024 --test src/ui/tipsters/profile_msg.rs -o target/profile_msg_test && ./target/profile_msg_test -q
    # Account's profile-link rules (7.1f, D-473), the desk's per-site patterns; no dependencies.
    cd app && rustc --edition 2024 --test src/ui/house/link_rules.rs -o target/link_rules_test && ./target/link_rules_test -q
    # The Privy signer's signature-slot filling (8.D, L8-3) on wire bytes; no dependencies.
    cd app && rustc --edition 2024 --test src/privy/wire.rs -o target/privy_wire_test && ./target/privy_wire_test -q

# 8.D (L8-3): rebuild app/assets/privy.js, the `window.SchedinaPrivy` bridge (Google login via
# Privy, its embedded Solana wallet), from app/privy/ with the pinned @privy-io/js-sdk-core and
# esbuild. The bundle is committed, so `dx` builds need no npm; run this after changing app/privy.
privy-bundle:
    cd app/privy && npm ci --ignore-scripts --no-audit --no-fund && npm run bundle
    @ls -l app/assets/privy.js | awk '{print "app/assets/privy.js: " $5 " bytes"}'

# Dev server with hot reload (dx 0.7)
app-serve:
    cd app && dx serve --platform web

# 6.2a-g: the phone gate on local builds (no desk, no chain): a demo build at 390, 1024 and 1440
# px (every screen; dark, no horizontal scroll, no trading words, honesty pill per D-311, values
# toned, tabular figures), a design build for the seeded slip, the Sportsbook, the spread and
# total tabs, the tipster and the cash-out celebration, then the dry run of devnet_e2e.mjs's UI
# path on both (e2e_dry.mjs). Screenshots and reports in app/target/phone-gate; servers stopped
# on exit. Env: PLAYWRIGHT_CORE, CHROME_PATH, SKIP_BUILD=1.
phone-gate:
    app/scripts/local_gates.sh phone-gate

# 6.2a-g: the design build served locally, every screen shot at the mockup's frames, and the
# side-by-side review page for sign-off (each screen next to its mockup PNG, a pixel-difference
# score, a note per screen); 7.2a-g: the parity page of the eight mockup screens at their frames
# (parity.html) and its summary (parity.md: score, matches, accepted differences with decision
# ids). Prints the pages' paths (app/scripts/visual/out/, gitignored).
visual:
    app/scripts/local_gates.sh visual

# --- devnet -------------------------------------------------------------

# Devnet deploy (step 3.B, docs/deploy.md): build the --features devnet .so, deploy or upgrade it,
# init_config, faucet the demo wallets, fund the vault. Idempotent; refuses mainnet. Key paths come
# from .env (paths only); from a worktree add SCHEDINA_ENV_FILE=… PROGRAM_KEYPAIR=… in front.
# Args pass through (--skip-build, --deploy-only, --close-buffer, --max-len N, --rpc-url URL).
deploy-devnet *args:
    scripts/deploy.sh --cluster devnet {{args}}

# Config, vault, pendings and tickets of the devnet program (read only)
crank-status-devnet:
    cargo run --quiet --locked -p desk --bin crank -- --url devnet status

# --- data ---------------------------------------------------------------

# NBA tape capture (step 0.D): stdlib-only recorder, args go to the script
tape-record *args:
    python3 -I scripts/record_tape.py {{args}}

# Plan 3.C (docs/demo-devnet.md): the app on devnet with the `demo` desk. Builds the app against a
# local desk (devnet program, public RPC, crank/quote-signer key path from .env) and the Pascal mock
# on the demo tape at 60x, serves it on http://localhost until Ctrl-C. `--e2e` instead runs the
# live-desk phone gate and the browser-only lifecycle (fresh burner wallet funded with ≤ 0.05 SOL
# from demo-3) with screenshots and signatures in docs/demo-devnet/, then stops everything.
# From a worktree add SCHEDINA_ENV_FILE=<main checkout>/.env in front.
demo-devnet *args:
    scripts/demo_devnet.sh {{args}}

# L5-2 (docs/demo.md): the web app on devnet, live by default (Pascal's NBA and soccer card through
# the mock; replay of the 28 Oct tape, with a notice, when no game is left), the mock's,
# the desk's and the static server's logs streamed here as [mock] [desk] [app], a banner with the
# URLs and a resolve command per game; Ctrl-C stops everything. mockBONK pays through the devnet
# mock AMM. Args: --replay, --localnet, --e2e, --record DIR, --skip-build.
# From a worktree add SCHEDINA_ENV_FILE=<main checkout>/.env in front.
demo *args:
    scripts/demo.sh {{args}}

# --- docs ---------------------------------------------------------------

# The docs/ pages as an mdbook (book.toml), opened in the browser: `cargo install mdbook mdbook-katex`.
# Output in target/book.
docs:
    mdbook build --open

# The book with live reload on http://localhost:3000
docs-serve:
    mdbook serve --open

# Writes docs/architecture.svg and its copy for the deck, slides/public/architecture.svg.
# The isometric architecture drawing
architecture:
    python3 scripts/architecture.py

# Writes docs/lifecycle.svg and its copy for the deck, slides/public/lifecycle.svg.
# The isometric bet lifecycle drawing
lifecycle:
    python3 scripts/lifecycle.py

# Slidev on http://localhost:3030 (presenter view with notes: /presenter). Installs on first run.
# The presentation
slides:
    cd slides && ([ -d node_modules ] || npm install) && npm run dev
