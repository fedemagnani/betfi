#!/usr/bin/env bash
# Plan 3.A: the whole Schedina lifecycle on localnet with the Pascal mock (`dev` profile).
#
#   just e2e                       (builds the devnet-feature .so and the binaries, then this)
#   scripts/e2e_lifecycle.sh       (E2E_KEEP=1 keeps the temp dir; E2E_SKIP_BUILD=1 skips cargo)
#
# From a clean temp ledger and temp keys: solana-test-validator (the `--features devnet` .so as
# an upgradeable program, SIMD-0500 deactivated) → crank init-config / faucet / fund-vault →
# pascal-mock (permits verified, the hedger's trading key registered, --speed 60, the demo tape)
# → the desk (PROFILE=dev, every key from the temp dir) → e2e-user: ticket A quote → open_pending
# → hedge → finalize → leg 1 resolves → roll → cash-out quote → cash_out; ticket B → settle;
# ticket C (6.0b), a spread leg and a total leg → settle, on a synthetic copy of the tape that
# also lists the real 28 Oct spreads and totals of PHI @ IND and ORL @ TOR with books
# (`e2e-user synth-tape`; E2E_LINES=0 runs the plain tape and skips C and D); ticket D (7.0b),
# an NBA winner leg and an EPL draw leg → settle, the EPL match (real 28 Oct listing of Aston
# Villa – Fulham, moved onto the tape's night) also on the synthetic tape; with the vault and hedge
# checkpoints, and a desk crash (SIGKILL) and restart between the mints
# and the resolutions (E2E_DESK_CRASH=0 skips it). Then the ticket timeline, the log checks, and every
# process stopped (trap, on every exit path). Localnet and the mock only (L3-1, L3-2).
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=scripts/e2e/lib.sh
source "$ROOT/scripts/e2e/lib.sh"

PROGRAM_ID=HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW
SIMD_0500=B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g # no new SBPF v0-v2 deployments (D-006)
SO=${SCHEDINA_SO:-$ROOT/programs/target/deploy-devnet/schedina.so}
TAPE=${E2E_TAPE:-$ROOT/tapes/nba-2026-10-28}
BIN=$ROOT/target/debug

e2e_guard_env
for tool in solana-test-validator solana solana-keygen python3 curl; do
  command -v "$tool" >/dev/null || e2e_die "$tool is not on PATH"
done

TMP=$(mktemp -d "${TMPDIR:-/tmp}/schedina-e2e.XXXXXX")
KEYS=$TMP/keys
mkdir -p "$KEYS" "$TMP/home"
chmod 700 "$KEYS"
OK=0
on_exit() {
  local code=$?
  trap - EXIT INT TERM
  e2e_cleanup
  if [ "$OK" = 1 ] && [ "${E2E_KEEP:-0}" != 1 ]; then
    rm -rf "$TMP"
  else
    e2e_log "logs and ledger kept in $TMP"
  fi
  if [ "$OK" = 1 ]; then e2e_log "E2E OK"; else e2e_log "E2E FAILED (exit $code)"; fi
  exit "$code"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# --- build ------------------------------------------------------------------------------------
if [ "${E2E_SKIP_BUILD:-0}" != 1 ]; then
  if [ ! -f "$SO" ]; then
    e2e_log "building the devnet-feature program (cargo build-sbf)"
    (cd "$ROOT/programs" && cargo build-sbf --manifest-path schedina/Cargo.toml --features devnet \
      --sbf-out-dir target/deploy-devnet) >"$TMP/build-sbf.log" 2>&1 ||
      e2e_die "cargo build-sbf failed: $TMP/build-sbf.log"
    rm -f "$ROOT/programs/target/deploy-devnet/schedina-keypair.json"
  fi
  e2e_log "building desk, crank, e2e-user, pascal-mock"
  (cd "$ROOT" && cargo build --locked -p desk --bins -p pascal-mock) >"$TMP/build.log" 2>&1 ||
    e2e_die "cargo build failed: $TMP/build.log"
fi
[ -f "$SO" ] || e2e_die "$SO missing (just e2e builds it)"
for b in desk crank e2e-user pascal-mock; do
  [ -x "$BIN/$b" ] || e2e_die "$BIN/$b missing (cargo build -p desk --bins -p pascal-mock)"
done

# --- keys (L3-2: all generated here, none reused) ---------------------------------------------
for k in authority crank quote-signer user hedger-owner hedger-trading builder sponsor; do
  e2e_keygen "$KEYS/$k.json"
done
AUTH_PK=$(solana-keygen pubkey "$KEYS/authority.json")
CRANK_PK=$(solana-keygen pubkey "$KEYS/crank.json")
QS_PK=$(solana-keygen pubkey "$KEYS/quote-signer.json")
USER_PK=$(solana-keygen pubkey "$KEYS/user.json")
HEDGER_PK=$(solana-keygen pubkey "$KEYS/hedger-owner.json")
TRADING_PK=$(solana-keygen pubkey "$KEYS/hedger-trading.json")
BUILDER_PK=$(solana-keygen pubkey "$KEYS/builder.json")
# L8: the desk's sponsor key (fees and rent of POST /relay transactions), apart from every other key.
SPONSOR_PK=$(solana-keygen pubkey "$KEYS/sponsor.json")
e2e_log "keys in $KEYS: authority $AUTH_PK, crank $CRANK_PK, quote signer $QS_PK, user $USER_PK"
e2e_log "hedger owner $HEDGER_PK, trading key $TRADING_PK, builder $BUILDER_PK, sponsor $SPONSOR_PK"

# --- ports ------------------------------------------------------------------------------------
VP=$(e2e_free_range)
RPC=http://127.0.0.1:$VP
e2e_log "validator $RPC (ports $VP-$((VP + 40)))"

# A clean environment for every child: nothing from the caller's shell or ~/.config leaks in.
CLEAN_ENV=(env -i "PATH=$PATH" "HOME=$TMP/home" "TMPDIR=${TMPDIR:-/tmp}")

# --- validator --------------------------------------------------------------------------------
e2e_start validator "$TMP/validator.log" "${CLEAN_ENV[@]}" solana-test-validator \
  --ledger "$TMP/ledger" --quiet --bind-address 127.0.0.1 \
  --rpc-port "$VP" --faucet-port $((VP + 2)) --gossip-port $((VP + 3)) \
  --dynamic-port-range "$((VP + 4))-$((VP + 40))" \
  --upgradeable-program "$PROGRAM_ID" "$SO" "$AUTH_PK" \
  --deactivate-feature "$SIMD_0500"
VALIDATOR_PID=${E2E_PIDS[${#E2E_PIDS[@]} - 1]}
height_ok() { [ "$(solana -u "$RPC" block-height 2>/dev/null || echo 0)" -gt 1 ]; }
e2e_wait_for 90 validator "$VALIDATOR_PID" height_ok
for pk in "$AUTH_PK" "$CRANK_PK" "$USER_PK" "$SPONSOR_PK"; do
  solana -u "$RPC" airdrop -k "$KEYS/authority.json" 100 "$pk" >/dev/null ||
    e2e_die "airdrop to $pk"
done

crank_cli() {
  "${CLEAN_ENV[@]}" "$BIN/crank" --rpc "$RPC" --program-id "$PROGRAM_ID" \
    --authority "$KEYS/authority.json" "$@"
}
e2e_log "crank init-config"
crank_cli init-config --crank "$CRANK_PK" --quote-signer "$QS_PK" | tee "$TMP/init.log"
crank_cli faucet --to "$AUTH_PK" --usdc 10000 >/dev/null
crank_cli fund-vault --usdc 9000 | tail -1
crank_cli faucet --to "$USER_PK" --usdc 100 >/dev/null
crank_cli status | tee "$TMP/status-start.log"

# --- Pascal mock ------------------------------------------------------------------------------
# Permit verification is on by default; only the hedger's trading key may sign for it. Leg
# resolutions come from e2e-user's /mock/resolve calls (deterministic), not from the tape's
# wall+40s script, which a slow first build could race.
# Ports from the kernel (ephemeral range, disjoint from the validator's block), picked right
# before each bind so nothing else can take them in between.
MOCK_TAPE=$TAPE
LINES=0
if [ "${E2E_LINES:-1}" = 1 ]; then
  MOCK_TAPE=$TMP/tape
  "$BIN/e2e-user" synth-tape --from "$TAPE" \
    --lines "$ROOT/fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json" \
    --soccer "$ROOT/fixtures/soccer/markets_2026-10-28_soccer.TRIMMED.json" --out "$MOCK_TAPE" ||
    e2e_die "synth-tape failed"
  LINES=1
fi
MOCK_PORT=$(e2e_free_port)
MOCK=http://127.0.0.1:$MOCK_PORT
e2e_start mock "$TMP/mock.log" "${CLEAN_ENV[@]}" RUST_LOG=info NO_COLOR=1 "$BIN/pascal-mock" \
  --tape "$MOCK_TAPE" --speed 60 --no-script --bind "127.0.0.1:$MOCK_PORT" \
  --trading-key "$HEDGER_PK=$TRADING_PK"
MOCK_PID=${E2E_PIDS[${#E2E_PIDS[@]} - 1]}
e2e_wait_for 30 mock "$MOCK_PID" curl -fsS "$MOCK/api/v1/time"

# --- desk -------------------------------------------------------------------------------------
DB=$TMP/desk.sqlite
DESK_PORT=$(e2e_free_port)
[ "$DESK_PORT" != "$MOCK_PORT" ] || DESK_PORT=$(e2e_free_port)
DESK=http://127.0.0.1:$DESK_PORT
e2e_log "mock $MOCK, desk $DESK"
DESK_RUN=0
start_desk() {
  DESK_RUN=$((DESK_RUN + 1))
  e2e_start desk "$TMP/desk-$DESK_RUN.log" "${CLEAN_ENV[@]}" RUST_LOG=info \
    PROFILE=dev DESK_LISTEN="127.0.0.1:$DESK_PORT" DESK_DB="$DB" \
    PASCAL_DATA_URL="$MOCK" PASCAL_TRADE_URL="$MOCK" \
    PASCAL_HEDGER_OWNER="$HEDGER_PK" PASCAL_HEDGER_TRADING_KEY_PATH="$KEYS/hedger-trading.json" \
    PASCAL_BUILDER_PUBKEY="$BUILDER_PK" \
    SOLANA_RPC_URL="$RPC" PROGRAM_ID="$PROGRAM_ID" SCHEDINA_KEYS_DIR="$KEYS" \
    CRANK_KEYPAIR_PATH="$KEYS/crank.json" QUOTE_SIGNER_KEYPAIR_PATH="$KEYS/quote-signer.json" \
    AUTHORITY_KEYPAIR_PATH="$KEYS/authority.json" SPONSOR_KEYPAIR_PATH="$KEYS/sponsor.json" \
    CRANK_RESOLVE_POLL_S=2 CRANK_RESOLVED_SWEEP_S=10 CRANK_RESOLVE_AFTER_S=0 CRANK_TICK_MS=500 \
    INDEXER_RESYNC_S=10 HEDGE_ROLL_TIMEOUT_MS=3000 \
    "$BIN/desk"
  DESK_PID=${E2E_PIDS[${#E2E_PIDS[@]} - 1]}
  e2e_wait_for 60 desk "$DESK_PID" curl -fsS "$DESK/health"
  e2e_wait_for 60 "desk catalogue" "$DESK_PID" curl -fsS "$DESK/card?sport=nba&date=2026-10-28"
}
start_desk
# L8: the relay is on and names the sponsor key generated above.
RELAY=$(curl -fsS "$DESK/relay") || e2e_die "GET /relay failed: the sponsored-transaction relay is off"
case "$RELAY" in
  *"\"sponsor\":\"$SPONSOR_PK\""*) e2e_log "relay on: sponsor $SPONSOR_PK" ;;
  *) e2e_die "GET /relay names another sponsor: $RELAY" ;;
esac

# --- the scripted user ------------------------------------------------------------------------
# Phase open: tickets A and B minted. Then (unless E2E_DESK_CRASH=0) the desk is killed with
# SIGKILL and restarted on the same database, as the runbook's "Desk crash" says. Phase close:
# the restarted desk must know both tickets and its held, then A rolls and cashes out, B settles.
user_run() {
  "${CLEAN_ENV[@]}" "$BIN/e2e-user" run --desk "$DESK" --mock "$MOCK" --rpc "$RPC" \
    --user "$KEYS/user.json" --hedger "$HEDGER_PK" --program-id "$PROGRAM_ID" \
    --lines "$LINES" --soccer "$LINES" --state "$TMP/e2e-state.json" "$@" | tee -a "$TMP/e2e-user.log"
  return "${PIPESTATUS[0]}"
}
set +e
user_run --phase open
USER_RC=$?
set -e
if [ "$USER_RC" = 0 ] && [ "${E2E_DESK_CRASH:-1}" = 1 ]; then
  e2e_log "crashing the desk (SIGKILL, pid $DESK_PID) and restarting it on the same database"
  kill -KILL "$DESK_PID"
  while e2e_alive "$DESK_PID"; do sleep 0.1; done
  start_desk
fi
if [ "$USER_RC" = 0 ]; then
  set +e
  user_run --phase close
  USER_RC=$?
  set -e
fi
cat "$TMP"/desk-*.log >"$TMP/desk.log"

echo
"$BIN/e2e-user" timeline --db "$DB" | tee "$TMP/timeline.log" || e2e_log "timeline failed"
echo
crank_cli status | tee "$TMP/status-end.log"
curl -fsS "$DESK/metrics" >"$TMP/metrics.txt" || true

# --- log checks -------------------------------------------------------------------------------
# Run whatever e2e-user said, so a failure names its cause. No rate limiting and no permit
# rejection anywhere (desk JSON logs, mock logs, the desk's Pascal error counter, the hedge
# order journal); no `held` drift fixed by reconciliation; no desk or mock errors.
FAILS=()
[ "$USER_RC" = 0 ] || FAILS+=("e2e-user failed (exit $USER_RC)")
BAD='RATE_LIMITED|UNAUTHORIZED|Too Many Requests|(http_)?status"?[=:] ?"?429([^0-9]|$)|HTTP 429'
if grep -E -i -n "$BAD" "$TMP/desk.log" "$TMP/mock.log" >"$TMP/bad.log"; then
  cut -c1-300 "$TMP/bad.log" | head -10
  FAILS+=("429 / RATE_LIMITED / UNAUTHORIZED in the logs ($(wc -l <"$TMP/bad.log" | tr -d ' ') lines)")
fi
if grep -E 'pascal_errors_total\{code="(RATE_LIMITED|UNAUTHORIZED)"\}' "$TMP/metrics.txt"; then
  FAILS+=("the desk counted RATE_LIMITED / UNAUTHORIZED Pascal errors")
fi
if grep -n 'held reconciled from /accounts' "$TMP/desk.log" | cut -c1-300 | head -5 | grep .; then
  FAILS+=("the desk's held drifted from the mock's account (reconciled)")
fi
if grep -n '"level":"ERROR"' "$TMP/desk.log" | cut -c1-300 | head -10 | grep .; then
  FAILS+=("the desk logged errors")
fi
if grep -n -E ' (ERROR|WARN) ' "$TMP/mock.log" | cut -c1-300 | head -10 | grep .; then
  FAILS+=("the mock logged warnings or errors")
fi
# (Read-only while the desk still has the WAL open; a stopped desk's DB needs ?immutable=1.)
if command -v sqlite3 >/dev/null; then
  if N=$(sqlite3 "file:$DB?mode=ro" "SELECT COUNT(*) FROM hedge_orders WHERE status IN ('rejected','error')
      AND (error LIKE '%UNAUTHORIZED%' OR error LIKE '%RATE_LIMITED%' OR error LIKE '%429%')" 2>&1); then
    [ "$N" = 0 ] || FAILS+=("$N hedge orders were rejected as UNAUTHORIZED / RATE_LIMITED")
  else
    FAILS+=("cannot read hedge_orders: $N")
  fi
fi
for pid in "$VALIDATOR_PID" "$MOCK_PID" "$DESK_PID"; do
  e2e_alive "$pid" || FAILS+=("a service (pid $pid) died during the run")
done
if [ "${#FAILS[@]}" -gt 0 ]; then
  for f in "${FAILS[@]}"; do e2e_log "FAIL: $f"; done
  e2e_die "${#FAILS[@]} check(s) failed; logs in $TMP"
fi

ORDERS=$(grep -c '"target":"schedina::signed_order"' "$TMP/desk.log" || true)
FILLS=$(grep -c 'pascal_mock::api: order' "$TMP/mock.log" || true)
e2e_log "summary: $(grep '^E2E PASS' "$TMP/e2e-user.log")"
e2e_log "summary: $ORDERS signed hedge orders logged by the desk, $FILLS orders filled or rested on the mock, desk runs $DESK_RUN; no 429, no UNAUTHORIZED, no desk or mock errors"
OK=1
