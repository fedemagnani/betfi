#!/usr/bin/env bash
# `just demo` (L5-2, docs/demo.md): the web app on devnet with the `demo` desk and the Pascal
# mock, their logs streamed to this terminal. A thin wrapper over scripts/demo_devnet.sh.
#
#   just demo [--replay] [--localnet] [--e2e] [--record DIR] [--skip-build] [--no-open]
#
#   (default)      live: every game Pascal lists that is not over, whatever its day, and
#                  Pascal's books through `pascal-mock --live` (reads only; every hedge fills
#                  in the mock, mode M). No game (NBA or soccer) left to start, or
#                  Pascal's read API unreachable: a clear notice, then the replay below instead. DEMO_SLATE_DATE=YYYY-MM-DD shows one US night only.
#   --replay       the 28 Oct 2026 tape at 60x (legs resolve through /mock/resolve).
#   --localnet     a private solana-test-validator instead of devnet (schedina and mock-amm
#                  loaded, initialised, funded and set up; every key generated). Build the .so
#                  files first: `just e2e` and `just swap-chain-test` leave them in
#                  programs/target/deploy-devnet/.
#   --e2e          after start-up, the browser checks of scripts/demo_devnet.sh --e2e (a burner
#                  funded with DEMO_FUND_SOL ≤ 0.05 SOL from demo-3.json), then stop; E2E_TIP,
#                  E2E_COPY (+ FUND2_SOL, both burners ≤ 0.05 SOL), E2E_BONK, E2E_SOL (after
#                  `crank amm-peg --from-fx`), E2E_SOCCER, BONK_AMOUNT,
#                  TIPSTER_SHARE_BPS pass through to app/scripts/devnet_e2e.mjs.
#   --record DIR   live only: also record the session as a tape in DIR (no replay fallback).
#   --no-open      do not open the app in the browser once it is up (opened by default,
#                  except with --e2e).
#
# Logs: [mock] [desk] [app] prefixes, colour on a TTY (NO_COLOR=1 turns it off), RPC and Pascal
# URLs redacted. Filters: DEMO_MOCK_LOG, DEMO_DESK_LOG (RUST_LOG syntax, default info);
# DEMO_LOG_QUIET=<regex> hides matching lines (default: the hedge desk's warnings about earlier runs'
# tickets; empty shows all); DEMO_LOG_BUS=1 shows the desk's raw bus events (else
# only the feed sentences); DEMO_LOG_WIDTH (240, 0 = no cut). Ctrl-C stops every process started.
# From a worktree: SCHEDINA_ENV_FILE=<main checkout>/.env just demo.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
LIVE=1
RECORD=
OPEN=1
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --replay) LIVE=0; shift ;;
    --localnet|--e2e|--skip-build) ARGS+=("$1"); shift ;;
    --record) RECORD=${2:?--record needs a directory}; shift 2 ;;
    --no-open) OPEN=0; shift ;;
    -h|--help) sed -n 2,30p "$0"; exit 0 ;;
    *) echo "demo: unknown flag $1 (--replay, --localnet, --e2e, --record DIR, --skip-build, --no-open)" >&2; exit 2 ;;
  esac
done
if [ -n "$RECORD" ]; then
  [ "$LIVE" = 1 ] || { echo "demo: --record records a live session; drop --replay" >&2; exit 2; }
  export DEMO_RECORD=$RECORD
fi
if [ "$LIVE" = 1 ]; then
  ARGS+=(--live)
  [ -n "$RECORD" ] || ARGS+=(--fallback)
fi
export DEMO_STREAM=1
export DEMO_OPEN=$OPEN
exec "$ROOT/scripts/demo_devnet.sh" ${ARGS[@]+"${ARGS[@]}"}
