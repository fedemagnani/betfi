#!/usr/bin/env bash
# Build the `--features devnet` schedina .so (SBPF v0; it carries the mock-USDC faucet), deploy or
# upgrade it, then init_config, mock-USDC faucet claims for the demo wallets and the vault funder,
# and fund_vault. Plan steps 0.B and 3.B; runbook and the devnet record in docs/deploy.md.
#
#   scripts/deploy.sh --cluster localnet|devnet [--program schedina|mock-amm] [--rpc-url URL]
#                     [--skip-build] [--max-len BYTES] [--deploy-only] [--close-buffer]
#
#   --program NAME   schedina (default) or mock-amm (5.0a, L5-1): the mock AMM's .so (plain
#                    `cargo build-sbf`, SBPF v0), program keypair MOCK_AMM_PROGRAM_KEYPAIR
#                    (default keys/mock-amm-program-keypair.json), the same upgrade authority;
#                    after the deploy `crank amm-setup` (AmmConfig, mockBONK and mockSOL pools
#                    against schedina's mock USDC, the lookup table). Needs schedina deployed and
#                    initialised (its program keypair still names PROGRAM_ID).
#   --max-len BYTES  program-data size of a first deploy (schedina: 589824 = 576 KiB, D-130;
#                    mock-amm: the .so + 12%, rounded up to 1 KiB); ignored on upgrades, which
#                    auto-extend when the .so outgrows it.
#   --deploy-only    stop after the deploy/upgrade (no init_config, faucet, funding, amm-setup).
#   --close-buffer   abandon a failed deploy: close its buffer (rent back to the authority),
#                    delete the buffer key, and exit.
#
# SOL limits for mock-amm (L5-1), checked before anything is sent and again at the end: the run
# spends at most MAX_SPEND_SOL (3.2), holds at most MAX_PEAK_SOL (5.5) at once (buffer and
# program data counted together), and leaves the authority at least KEEP_SOL (9). Schedina keeps
# its 1 SOL floor and no caps (L3-3).
#
# Idempotent: a re-run skips the deploy when the cluster already runs this exact .so (upgrades it
# otherwise), skips init_config when Config exists (and checks its keys), and tops up the demo
# wallets and the vault only up to their targets. A failed deploy is resumed by running again
# (same buffer key, only missing chunks rewritten).
#
# Devnet: key paths come from the environment or the gitignored .env (paths only, lib.sh); the
# authority must be funded beforehand and the script stops with the exact shortfall otherwise.
# Localnet expects a running solana-test-validator with SIMD-0500 deactivated (D-006:
# --deactivate-feature B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g) and airdrops the authority.
#
# Signers are always explicit (-k / --upgrade-authority / crank --authority); the Solana CLI
# default signer and configured URL are never used. Mainnet is refused (lib.sh, crank). Output is
# redacted (no RPC query string) and appended to programs/target/deploy-devnet/deploy-<cluster>.log.
set -euo pipefail

CLUSTER=""; RPC_URL="${RPC_URL:-}"; SKIP_BUILD=0; DEPLOY_ONLY=0; CLOSE_BUFFER=0
MAX_LEN="${MAX_LEN:-}"; PROGRAM=schedina
while [ $# -gt 0 ]; do
  case "$1" in
    --cluster) CLUSTER="${2:?--cluster needs a value}"; shift 2 ;;
    --program) PROGRAM="${2:?--program needs a value}"; shift 2 ;;
    --rpc-url) RPC_URL="${2:?--rpc-url needs a value}"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --max-len) MAX_LEN="${2:?--max-len needs a value}"; shift 2 ;;
    --deploy-only) DEPLOY_ONLY=1; shift ;;
    --close-buffer) CLOSE_BUFFER=1; shift ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done
case "$PROGRAM" in schedina|mock-amm) ;; *) echo "--program must be schedina or mock-amm" >&2; exit 2 ;; esac
case "$MAX_LEN" in '') ;; *[!0-9]*) echo "--max-len must be a byte count" >&2; exit 2 ;; esac
# shellcheck source=scripts/lib.sh
. "$(dirname "$0")/lib.sh"

sol_to_lamports() { awk -v s="$1" 'BEGIN { printf "%.0f", s * 1e9 }'; }
# What this run deploys. Schedina's ID comes from lib.sh (PROGRAM_KEYPAIR); mock-amm's from its
# own keypair, and a MOCK_AMM_PROGRAM_ID in the environment must agree with it.
if [ "$PROGRAM" = schedina ]; then
  DEPLOY_KEYPAIR="$PROGRAM_KEYPAIR"; DEPLOY_ID="$PROGRAM_ID"
  DEPLOY_MANIFEST="$PROGRAM_MANIFEST"; DEPLOY_SO="$SO"; DEPLOY_FEATURES=(--features devnet)
  # Only the --features devnet build has faucet_usdc (L3-3).
  DEPLOY_MARKER=FaucetUsdc; DEPLOY_LIB="$PROGRAMS_DIR/schedina/src/lib.rs"
  BUFFER_NAME="deploy-buffer-$CLUSTER.json"
  MAX_LEN="${MAX_LEN:-589824}"
  KEEP_LAMPORTS=1000000000; MAX_SPEND_LAMPORTS=0; MAX_PEAK_LAMPORTS=0
else
  need_file "$MOCK_AMM_PROGRAM_KEYPAIR" "mock-amm program keypair (set MOCK_AMM_PROGRAM_KEYPAIR)"
  DEPLOY_KEYPAIR="$MOCK_AMM_PROGRAM_KEYPAIR"; DEPLOY_ID="$(pubkey_of "$MOCK_AMM_PROGRAM_KEYPAIR")"
  if [ -n "${MOCK_AMM_PROGRAM_ID:-}" ] && [ "$MOCK_AMM_PROGRAM_ID" != "$DEPLOY_ID" ]; then
    echo "MOCK_AMM_PROGRAM_ID=$MOCK_AMM_PROGRAM_ID but $MOCK_AMM_PROGRAM_KEYPAIR is $DEPLOY_ID" >&2; exit 1
  fi
  DEPLOY_MANIFEST="$PROGRAMS_DIR/mock-amm/Cargo.toml"; DEPLOY_SO="$SO_DIR/mock_amm.so"; DEPLOY_FEATURES=()
  DEPLOY_MARKER=SetLookupTable; DEPLOY_LIB="$PROGRAMS_DIR/mock-amm/src/lib.rs"
  BUFFER_NAME="deploy-buffer-mock-amm-$CLUSTER.json"
  KEEP_LAMPORTS=$(sol_to_lamports "${KEEP_SOL:-9}")
  MAX_SPEND_LAMPORTS=$(sol_to_lamports "${MAX_SPEND_SOL:-3.2}")
  MAX_PEAK_LAMPORTS=$(sol_to_lamports "${MAX_PEAK_SOL:-5.5}")
fi

# Config parameters: spec section 3 "Caps" and section 4 `Config` (D-064/D-111 copy fee).
MAX_LEGS="${MAX_LEGS:-5}"
MIN_STAKE_USDC="${MIN_STAKE_USDC:-5}"
MAX_PAYOUT_USDC="${MAX_PAYOUT_USDC:-500}"
COPY_FEE_BPS="${COPY_FEE_BPS:-200}"
TIPSTER_SHARE_BPS="${TIPSTER_SHARE_BPS:-8000}"
# Floor withdraw_house cannot take Vault.free below: two max-payout tickets.
RESERVE_USDC="${RESERVE_USDC:-1000}"
# Targets: the vault holds at least VAULT_USDC (10 max-payout tickets), each demo wallet at least
# DEMO_USDC (one faucet tap, D-103). One faucet claim is capped at 10,000 USDC per owner per hour.
VAULT_USDC="${VAULT_USDC:-5000}"
DEMO_USDC="${DEMO_USDC:-1000}"
FAUCET_MAX_U=10000000000
# SOL safety: the authority keeps KEEP_LAMPORTS after deploy, init and funding (set above);
# INIT_BUDGET covers init_config's five accounts and four faucet ATAs + claims (~0.02 SOL), or
# amm-setup's ~17 accounts and its lookup table (~0.03 SOL).
INIT_BUDGET_LAMPORTS=50000000
# A demo wallet below this gets one airdrop attempt (non-fatal on devnet).
DEMO_MIN_LAMPORTS=50000000
SIMD_0500=B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g

mkdir -p "$SO_DIR"
LOG="$SO_DIR/deploy-$CLUSTER.log"
exec > >(redact_stream | tee -a "$LOG") 2>&1
log "=== deploy.sh $(date -u +%Y-%m-%dT%H:%M:%SZ) cluster $CLUSTER ($RPC_SHOWN), program $PROGRAM"

S=(--url "$RPC_URL" -k "$AUTHORITY_KEYPAIR")
SIGS=()
record_sig() {
  case "$2" in ''|null) echo "no transaction signature in the output of $1" >&2; exit 1 ;; esac
  SIGS+=("$1|$2"); log "signature $1: $2"
}

need_file "$AUTHORITY_KEYPAIR" "authority keypair (set AUTHORITY_KEYPAIR_PATH)"
AUTHORITY_PUBKEY=$(pubkey_of "$AUTHORITY_KEYPAIR")
# The binary must declare the same ID, or every instruction fails with DeclaredProgramIdMismatch.
DECLARED_ID=$(sed -n 's/^declare_id!("\([1-9A-HJ-NP-Za-km-z]*\)");.*/\1/p' "$DEPLOY_LIB")
if [ "$DECLARED_ID" != "$DEPLOY_ID" ]; then
  echo "declare_id! in ${DEPLOY_LIB#"$ROOT"/} is '$DECLARED_ID', keypair is $DEPLOY_ID" >&2; exit 1
fi
log "program $PROGRAM $DEPLOY_ID (keypair $(tilde "$DEPLOY_KEYPAIR")); authority $AUTHORITY_PUBKEY ($(tilde "$AUTHORITY_KEYPAIR"))"

# Resumable deploy buffer, kept outside the repo until the deploy succeeds (one per program).
mkdir -p "$KEYS_DIR"
BUFFER="$KEYS_DIR/$BUFFER_NAME"
buffer_lamports() {
  [ -f "$BUFFER" ] || { echo 0; return; }
  local out
  out=$(solana account "${S[@]}" --output json "$(pubkey_of "$BUFFER")" 2>&1) || {
    case "$out" in *AccountNotFound*|*"Unable to find"*) echo 0; return ;; esac
    echo "cannot read the deploy buffer: $out" >&2; return 1
  }
  printf '%s' "$out" | jq -r .account.lamports
}
close_buffer() {
  local addr lam
  addr=$(pubkey_of "$BUFFER"); lam=$(buffer_lamports)
  if [ "$lam" -gt 0 ]; then
    solana program close "${S[@]}" --authority "$AUTHORITY_KEYPAIR" --recipient "$AUTHORITY_PUBKEY" "$addr"
    log "closed buffer $addr: $(sol_of "$lam") SOL back to the authority"
  fi
  rm -f "$BUFFER"
}

if [ "$CLOSE_BUFFER" = 1 ]; then
  if [ -f "$BUFFER" ]; then close_buffer; else log "no buffer key at $(tilde "$BUFFER")"; fi
  exit 0
fi

declare -a BAL_NAMES BAL_KEYS BAL_BEFORE
DEMO_PUBKEYS=()
if [ "$PROGRAM" = schedina ]; then
  # Signers stored in Config and the demo wallets.
  need_file "$CRANK_KEYPAIR" "crank keypair (set CRANK_KEYPAIR_PATH)"
  need_file "$QUOTE_SIGNER_KEYPAIR" "quote-signer keypair (set QUOTE_SIGNER_KEYPAIR_PATH)"
  CRANK_PUBKEY=$(pubkey_of "$CRANK_KEYPAIR")
  QUOTE_SIGNER_PUBKEY=$(pubkey_of "$QUOTE_SIGNER_KEYPAIR")
  for kp in "${DEMO_KEYPAIRS[@]}"; do need_file "$kp" "demo wallet"; DEMO_PUBKEYS+=("$(pubkey_of "$kp")"); done
  BAL_NAMES=(authority crank/quote-signer demo-1 demo-2 demo-3)
  BAL_KEYS=("$AUTHORITY_PUBKEY" "$CRANK_PUBKEY" "${DEMO_PUBKEYS[@]}")
  [ "$QUOTE_SIGNER_PUBKEY" = "$CRANK_PUBKEY" ] || log "quote signer $QUOTE_SIGNER_PUBKEY (separate from the crank)"
else
  # The mock AMM's deploy and set-up are paid by the authority alone.
  BAL_NAMES=(authority); BAL_KEYS=("$AUTHORITY_PUBKEY")
fi
for i in "${!BAL_KEYS[@]}"; do
  BAL_BEFORE[i]=$(lamports_of "${BAL_KEYS[i]}")
  log "SOL before: ${BAL_NAMES[i]} ${BAL_KEYS[i]} $(sol_of "${BAL_BEFORE[i]}")"
done

# Buffers of this authority other than ours belong to someone else (the key may be shared
# with another project): report them, never close them.
OTHER_BUFFERS=$(solana program show "${S[@]}" --buffers --buffer-authority "$AUTHORITY_PUBKEY" --output json \
  | jq -r '.buffers[].address')
OUR_BUFFER=""; [ -f "$BUFFER" ] && OUR_BUFFER=$(pubkey_of "$BUFFER")
for b in $OTHER_BUFFERS; do
  [ "$b" = "$OUR_BUFFER" ] || log "note: authority also owns buffer $b (not this script's; left alone)"
done

# --- build ---------------------------------------------------------------------------------
if [ "$SKIP_BUILD" = 0 ]; then
  log "building the $PROGRAM .so (cargo build-sbf${DEPLOY_FEATURES[*]+ ${DEPLOY_FEATURES[*]}}, SBPF v0)"
  (cd "$PROGRAMS_DIR" && cargo build-sbf --manifest-path "$DEPLOY_MANIFEST" \
      ${DEPLOY_FEATURES[@]+"${DEPLOY_FEATURES[@]}"} --sbf-out-dir "$SO_DIR")
  # cargo build-sbf writes a fresh random keypair next to the .so; it is not the program's.
  rm -f "$SO_DIR/schedina-keypair.json" "$SO_DIR/mock_amm-keypair.json"
fi
need_file "$DEPLOY_SO" "$PROGRAM .so (drop --skip-build)"
if ! LC_ALL=C grep -aq "$DEPLOY_MARKER" "$DEPLOY_SO"; then
  echo "$DEPLOY_SO lacks $DEPLOY_MARKER: not the $PROGRAM build this script deploys (L3-3)" >&2; exit 1
fi
SO_BYTES=$(wc -c < "$DEPLOY_SO" | tr -d ' ')
SO_SHA=$(shasum -a 256 "$DEPLOY_SO" | awk '{print $1}')
# mock-amm: --max-len = the .so + 12%, rounded up to 1 KiB (L5-1).
MAX_LEN="${MAX_LEN:-$(( (SO_BYTES * 112 / 100 + 1023) / 1024 * 1024 ))}"
# ELF e_flags (offset 48) is the SBPF version: 0 for v0. SIMD-0500 rejects new v0-v2 deploys.
SBPF=$(od -An -tu4 -j48 -N4 "$DEPLOY_SO" | tr -d ' ')
SIMD_STATUS=$(solana feature status "${S[@]}" --output json "$SIMD_0500" | jq -r '.features[0].status')
log ".so $SO_BYTES bytes, sha256 $SO_SHA, SBPF e_flags $SBPF; SIMD-0500 on $CLUSTER: $SIMD_STATUS"

# --- deploy or upgrade ---------------------------------------------------------------------
PROGRAM_DATA=""; DATA_LEN=0; PD_LAMPORTS=0
show_err=$(mktemp)
if SHOW=$(solana program show "${S[@]}" --output json "$DEPLOY_ID" 2>"$show_err"); then
  ONCHAIN_AUTH=$(printf '%s' "$SHOW" | jq -r .authority)
  PROGRAM_DATA=$(printf '%s' "$SHOW" | jq -r .programdataAddress)
  DATA_LEN=$(printf '%s' "$SHOW" | jq -r .dataLen)
  PD_LAMPORTS=$(printf '%s' "$SHOW" | jq -r .lamports)
  if [ "$ONCHAIN_AUTH" != "$AUTHORITY_PUBKEY" ]; then
    rm -f "$show_err"
    echo "$DEPLOY_ID upgrade authority is $ONCHAIN_AUTH, not $AUTHORITY_PUBKEY: refusing" >&2; exit 1
  fi
  MODE=upgrade
elif grep -q "Unable to find\|AccountNotFound" "$show_err"; then
  MODE=fresh
else
  cat "$show_err" >&2; rm -f "$show_err"; echo "cannot read $DEPLOY_ID" >&2; exit 1
fi
rm -f "$show_err"

DUMP="$SO_DIR/onchain-$PROGRAM-$CLUSTER.so"
rm -f "$DUMP"
if [ "$MODE" = upgrade ] && [ "$DATA_LEN" -ge "$SO_BYTES" ] \
   && solana program dump "${S[@]}" "$DEPLOY_ID" "$DUMP" >/dev/null \
   && head -c "$SO_BYTES" "$DUMP" | cmp -s - "$DEPLOY_SO"; then
  # (BSD cmp -n reports EOF on the shorter file instead of comparing its first n bytes, so the
  # dump, padded to the program-data size, is cut to the .so length first.)
  MODE=same
fi
rm -f "$DUMP"

if [ "$MODE" = same ]; then
  log "the deployed program already matches $DEPLOY_SO ($DATA_LEN-byte program data); deploy skipped"
  # A buffer left by an interrupted earlier run is ours and no longer needed.
  if [ -f "$BUFFER" ]; then close_buffer; fi
else
  # Checked only when something is sent, so a re-run against an unchanged program stays clean.
  if [ "$SBPF" -lt 3 ] && [ "$SIMD_STATUS" != inactive ]; then
    echo "SIMD-0500 is $SIMD_STATUS on $CLUSTER: an SBPF v$SBPF .so cannot be deployed; rebuild with --arch v3 (D-006)" >&2
    exit 1
  fi
  if [ "$MODE" = fresh ] && [ "$MAX_LEN" -lt "$SO_BYTES" ]; then
    echo "--max-len $MAX_LEN is below the .so size $SO_BYTES" >&2; exit 1
  fi
  # Fees: ~one write transaction per 1 KB chunk, doubled for re-signs, plus the final ones.
  FEES=$(( (SO_BYTES / 1000 + 20) * 5000 * 2 ))
  HAVE_BUFFER=$(buffer_lamports)
  if [ "$MODE" = fresh ]; then
    PD_RENT=$(rent_of $((MAX_LEN + 45)))
    PROG_RENT=$(rent_of 36)
    BUF_RENT=$(rent_of $((SO_BYTES + 37)))
    SPEND=$((PD_RENT + PROG_RENT + FEES + INIT_BUDGET_LAMPORTS))
    # Conservative peak: the buffer and the program data both funded at once.
    PEAK=$((BUF_RENT + PD_RENT + PROG_RENT + FEES))
    NEED=$((SPEND + KEEP_LAMPORTS))
    if [ "$MAX_PEAK_LAMPORTS" -gt 0 ] && [ "$PEAK" -gt "$NEED" ]; then NEED=$PEAK; fi
    log "first deploy: .so $SO_BYTES bytes, --max-len $MAX_LEN; program-data rent $PD_RENT + program $PROG_RENT lamports;" \
        "spends ~$(sol_of "$SPEND") SOL (peak $(sol_of "$PEAK") with the buffer);" \
        "need $(sol_of "$NEED") SOL incl. fees, set-up and the $(sol_of "$KEEP_LAMPORTS") SOL floor"
    DEPLOY_ARGS=(--max-len "$MAX_LEN")
  else
    BUF_RENT=$(rent_of $((SO_BYTES + 37)))
    EXT=0
    if [ "$SO_BYTES" -gt "$DATA_LEN" ]; then EXT=$(( $(rent_of $((SO_BYTES + 45))) - PD_LAMPORTS )); fi
    [ "$EXT" -gt 0 ] || EXT=0
    TRANSIENT=$((BUF_RENT + EXT + FEES))
    FLOOR=$((EXT + FEES + KEEP_LAMPORTS))
    NEED=$(( TRANSIENT > FLOOR ? TRANSIENT : FLOOR ))
    SPEND=$((EXT + FEES)); PEAK=$TRANSIENT
    log "upgrade: .so $SO_BYTES bytes into $DATA_LEN-byte program data; buffer rent $BUF_RENT (refunded)," \
        "extension $EXT lamports; need $(sol_of "$NEED") SOL"
    DEPLOY_ARGS=()
  fi
  # L5-1 caps (mock-amm): refuse before the first lamport moves.
  if [ "$MAX_SPEND_LAMPORTS" -gt 0 ] && [ "$SPEND" -gt "$MAX_SPEND_LAMPORTS" ]; then
    echo "the $MODE deploy would spend ~$(sol_of "$SPEND") SOL, over the $(sol_of "$MAX_SPEND_LAMPORTS") SOL cap: refusing" >&2; exit 1
  fi
  if [ "$MAX_PEAK_LAMPORTS" -gt 0 ] && [ "$PEAK" -gt "$MAX_PEAK_LAMPORTS" ]; then
    echo "the $MODE deploy would hold ~$(sol_of "$PEAK") SOL at its peak, over the $(sol_of "$MAX_PEAK_LAMPORTS") SOL cap: refusing" >&2; exit 1
  fi
  ensure_lamports "$AUTHORITY_PUBKEY" "$NEED" "$HAVE_BUFFER" "the $MODE deploy"

  if [ ! -f "$BUFFER" ]; then
    solana-keygen new --no-bip39-passphrase --silent -o "$BUFFER" >/dev/null
    chmod 600 "$BUFFER"
  fi
  log "deploying ($MODE, buffer $(pubkey_of "$BUFFER"), key $(tilde "$BUFFER"))"
  # Public devnet RPC: send through RPC rather than TPU/QUIC, which is often unreachable.
  if [ "$CLUSTER" = devnet ]; then DEPLOY_ARGS+=(--use-rpc); fi
  if ! DEPLOY_OUT=$(solana program deploy "${S[@]}" \
      --upgrade-authority "$AUTHORITY_KEYPAIR" \
      --program-id "$DEPLOY_KEYPAIR" \
      --buffer "$BUFFER" \
      --max-sign-attempts 50 \
      --output json \
      ${DEPLOY_ARGS[@]+"${DEPLOY_ARGS[@]}"} \
      "$DEPLOY_SO"); then
    echo "deploy failed. The buffer $(pubkey_of "$BUFFER") holds $(sol_of "$(buffer_lamports)") SOL:" \
         "re-run the same command to resume it, or run with --close-buffer to get the SOL back." >&2
    exit 1
  fi
  rm -f "$BUFFER"
  record_sig "deploy ($MODE)" "$(printf '%s' "$DEPLOY_OUT" | jq -r .signature)"
fi

SHOW=$(solana program show "${S[@]}" --output json "$DEPLOY_ID")
printf '%s\n' "$SHOW"
PROGRAM_DATA=$(printf '%s' "$SHOW" | jq -r .programdataAddress)
DATA_LEN=$(printf '%s' "$SHOW" | jq -r .dataLen)
[ "$(printf '%s' "$SHOW" | jq -r .authority)" = "$AUTHORITY_PUBKEY" ] \
  || { echo "upgrade authority changed under us" >&2; exit 1; }

# --- init_config, faucet, fund_vault -------------------------------------------------------
CONFIG=$(pda string:config)
VAULT=$(pda string:vault)
VAULT_USDC_ACC=$(pda string:vault_usdc)
ESCROW_USDC=$(pda string:escrow_usdc)
MINT=$(pda string:mock string:USDC)
EVENT_AUTHORITY=$(pda string:__event_authority)

CRANK=("${CARGO_TARGET_DIR:-$ROOT/target}/debug/crank" --rpc "$RPC_URL" --program-id "$PROGRAM_ID" \
       --authority "$AUTHORITY_KEYPAIR")
build_crank() {
  log "building the crank CLI"
  cargo build --quiet --locked --manifest-path "$ROOT/Cargo.toml" -p desk --bin crank
}

# --- mock-amm: amm-setup (5.0a) --------------------------------------------------------------
AMM_OUT=""
if [ "$PROGRAM" = mock-amm ] && [ "$DEPLOY_ONLY" = 0 ]; then
  build_crank
  # Idempotent: it reads first and sends only what is missing (a re-run prints "nothing sent").
  AMM_OUT=$("${CRANK[@]}" amm-setup --amm-program-id "$DEPLOY_ID")
  printf '%s\n' "$AMM_OUT"
  while read -r _tx step sig _rest; do
    record_sig "amm-setup $step" "$sig"
  done < <(printf '%s\n' "$AMM_OUT" | grep '^tx ')
fi

if [ "$PROGRAM" = schedina ] && [ "$DEPLOY_ONLY" = 0 ]; then
  build_crank
  usdc_str() { awk -v u="$1" 'BEGIN { printf "%d.%06d", int(u / 1000000), u % 1000000 }'; }

  STATUS=$("${CRANK[@]}" status)
  if printf '%s' "$STATUS" | grep -q "^slot .*: no config at"; then
    log "init_config: crank $CRANK_PUBKEY, quote signer $QUOTE_SIGNER_PUBKEY, max legs $MAX_LEGS," \
        "stake >= $MIN_STAKE_USDC, payout <= $MAX_PAYOUT_USDC, reserve $RESERVE_USDC USDC," \
        "copy fee $COPY_FEE_BPS bps, tipster share $TIPSTER_SHARE_BPS bps"
    OUT=$("${CRANK[@]}" init-config --crank "$CRANK_PUBKEY" --quote-signer "$QUOTE_SIGNER_PUBKEY" \
      --max-legs "$MAX_LEGS" --min-stake-usdc "$MIN_STAKE_USDC" --max-payout-usdc "$MAX_PAYOUT_USDC" \
      --reserve-usdc "$RESERVE_USDC" --copy-fee-bps "$COPY_FEE_BPS" --tipster-share-bps "$TIPSTER_SHARE_BPS")
    printf '%s\n' "$OUT"
    record_sig init_config "$(printf '%s' "$OUT" | awk '/^init_config /{print $2}')"
    STATUS=$("${CRANK[@]}" status)
  else
    log "Config $CONFIG exists; init_config skipped"
  fi
  # The keys in Config must be ours, or the desk cannot crank or sign quotes.
  field() { printf '%s\n' "$STATUS" | awk -v k="$1" '$1 == k {print $2; exit}'; }
  for pair in "authority:$AUTHORITY_PUBKEY" "crank:$CRANK_PUBKEY" "quote_signer:$QUOTE_SIGNER_PUBKEY" "usdc_mint:$MINT"; do
    got=$(field "${pair%%:*}")
    [ "$got" = "${pair#*:}" ] || { echo "Config.${pair%%:*} is '$got', expected ${pair#*:}" >&2; exit 1; }
  done
  want="max_legs $MAX_LEGS  max_payout $(usdc_str $((MAX_PAYOUT_USDC * 1000000)))  min_stake $(usdc_str $((MIN_STAKE_USDC * 1000000)))  reserve $(usdc_str $((RESERVE_USDC * 1000000)))"
  printf '%s\n' "$STATUS" | grep -qF "$want" \
    || log "WARNING: Config parameters differ from this script's ($want); change them with set_params"
  printf '%s\n' "$STATUS" | grep -qF "copy_fee_bps $COPY_FEE_BPS  tipster_share_bps $TIPSTER_SHARE_BPS" \
    || log "WARNING: Config fee parameters differ from $COPY_FEE_BPS / $TIPSTER_SHARE_BPS bps"

  # Demo wallets: at least DEMO_USDC mock USDC each (faucet paid by the authority) and some SOL.
  for i in "${!DEMO_PUBKEYS[@]}"; do
    who="${DEMO_PUBKEYS[i]}"; name="demo-$((i + 1))"
    have=$(token_amount "$(ata_of "$who" "$MINT")")
    if [ "$have" -lt $((DEMO_USDC * 1000000)) ]; then
      OUT=$("${CRANK[@]}" faucet --to "$who" --usdc "$DEMO_USDC")
      printf '%s\n' "$OUT"
      record_sig "faucet $name" "$(printf '%s' "$OUT" | awk '/^faucet /{sub(":", "", $2); print $2}')"
    else
      log "$name $who holds $(usdc_str "$have") mock USDC; faucet skipped"
    fi
    if [ "$(lamports_of "$who")" -lt "$DEMO_MIN_LAMPORTS" ]; then
      solana airdrop "${S[@]}" 0.5 "$who" || log "airdrop to $name failed (devnet faucet limits); fund it by hand"
    fi
  done

  # Vault: top up to VAULT_USDC from the authority's ATA, claiming the shortfall from the faucet.
  in_vault=$(token_amount "$VAULT_USDC_ACC")
  target=$((VAULT_USDC * 1000000))
  if [ "$in_vault" -lt "$target" ]; then
    gap=$((target - in_vault))
    mine=$(token_amount "$(ata_of "$AUTHORITY_PUBKEY" "$MINT")")
    if [ "$mine" -lt "$gap" ]; then
      claim=$((gap - mine))
      [ "$claim" -le "$FAUCET_MAX_U" ] || { echo "vault gap $(usdc_str "$claim") USDC exceeds one faucet claim" >&2; exit 1; }
      OUT=$("${CRANK[@]}" faucet --to "$AUTHORITY_PUBKEY" --usdc "$(usdc_str "$claim")")
      printf '%s\n' "$OUT"
      record_sig "faucet vault funder" "$(printf '%s' "$OUT" | awk '/^faucet /{sub(":", "", $2); print $2}')"
    fi
    OUT=$("${CRANK[@]}" fund-vault --usdc "$(usdc_str "$gap")")
    printf '%s\n' "$OUT"
    record_sig fund_vault "$(printf '%s' "$OUT" | awk '/^fund_vault /{sub(":", "", $2); print $2}')"
  else
    log "vault token account holds $(usdc_str "$in_vault") mock USDC; funding skipped"
  fi
  "${CRANK[@]}" status
fi

# --- record --------------------------------------------------------------------------------
explorer() {
  case "$CLUSTER" in
    devnet) printf 'https://explorer.solana.com/%s/%s?cluster=devnet' "$1" "$2" ;;
    *) printf '%s' "$2" ;;
  esac
}
echo
echo "| Role | Address |"
echo "| --- | --- |"
if [ "$PROGRAM" = mock-amm ]; then
  echo "| Program \`mock-amm\` | \`$DEPLOY_ID\` |"
  echo "| ProgramData ($DATA_LEN bytes; .so $SO_BYTES) | \`$PROGRAM_DATA\` |"
  echo "| Upgrade authority = \`AmmConfig.admin\` ($(tilde "$AUTHORITY_KEYPAIR")) | \`$AUTHORITY_PUBKEY\` |"
  echo "| Quote mint: schedina's mock USDC | \`$MINT\` |"
  # amm-setup's summary lines: "  amm_config X", "  lookup_table X (n addresses)", "  mockBONK mint X pool Y vaults A / B".
  printf '%s\n' "$AMM_OUT" | awk '
    $1 == "amm_config" { printf "| `AmmConfig` `[\"amm_config\"]` | `%s` |\n", $2 }
    $1 == "lookup_table" { printf "| Lookup table %s %s | `%s` |\n", $3, $4, $2 }
    $2 == "mint" && $4 == "pool" {
      printf "| %s mint `[\"mock\",\"%s\"]` | `%s` |\n", $1, substr($1, 5), $3
      printf "| %s pool `[\"pool\",mint]` | `%s` |\n", $1, $5
      printf "| %s pool vaults (mock, USDC) | `%s`, `%s` |\n", $1, $7, $9
    }
    $1 == "reserves" { line = $0; sub(/^ +/, "", line); r = r "\n- " line }
    END { if (r != "") print "\nReserves:" r }'
else
echo "| Program \`schedina\` | \`$PROGRAM_ID\` |"
echo "| ProgramData ($DATA_LEN bytes; .so $SO_BYTES) | \`$PROGRAM_DATA\` |"
echo "| Upgrade authority = \`Config.authority\` ($(tilde "$AUTHORITY_KEYPAIR")) | \`$AUTHORITY_PUBKEY\` |"
echo "| Crank ($(tilde "$CRANK_KEYPAIR")) | \`$CRANK_PUBKEY\` |"
echo "| Quote signer ($(tilde "$QUOTE_SIGNER_KEYPAIR")) | \`$QUOTE_SIGNER_PUBKEY\` |"
echo "| \`Config\` \`[\"config\"]\` | \`$CONFIG\` |"
echo "| \`Vault\` \`[\"vault\"]\` | \`$VAULT\` |"
echo "| Vault USDC \`[\"vault_usdc\"]\` | \`$VAULT_USDC_ACC\` |"
echo "| Escrow USDC \`[\"escrow_usdc\"]\` | \`$ESCROW_USDC\` |"
echo "| Mock USDC mint \`[\"mock\",\"USDC\"]\` | \`$MINT\` |"
echo "| Event authority \`[\"__event_authority\"]\` | \`$EVENT_AUTHORITY\` |"
for i in "${!DEMO_PUBKEYS[@]}"; do
  echo "| Demo wallet demo-$((i + 1)) ($(tilde "${DEMO_KEYPAIRS[i]}")) | \`${DEMO_PUBKEYS[i]}\` |"
done
fi
echo
for s in ${SIGS[@]+"${SIGS[@]}"}; do echo "- ${s%%|*}: $(explorer tx "${s#*|}")"; done
[ ${#SIGS[@]} -gt 0 ] || echo "- no transactions sent (everything already in place)"
echo
for i in "${!BAL_KEYS[@]}"; do
  after=$(lamports_of "${BAL_KEYS[i]}")
  log "SOL ${BAL_NAMES[i]} ${BAL_KEYS[i]}: $(sol_of "${BAL_BEFORE[i]}") -> $(sol_of "$after")"
done
AUTH_AFTER=$(lamports_of "$AUTHORITY_PUBKEY")
SPENT=$((BAL_BEFORE[0] - AUTH_AFTER))
log "authority spent $(sol_of "$SPENT") SOL this run"
if [ "$MAX_SPEND_LAMPORTS" -gt 0 ] && [ "$SPENT" -gt "$MAX_SPEND_LAMPORTS" ]; then
  echo "WARNING: this run spent $(sol_of "$SPENT") SOL, over the $(sol_of "$MAX_SPEND_LAMPORTS") SOL cap" >&2; exit 1
fi
if [ "$AUTH_AFTER" -lt "$KEEP_LAMPORTS" ]; then
  echo "WARNING: the authority holds $(sol_of "$AUTH_AFTER") SOL, below the $(sol_of "$KEEP_LAMPORTS") SOL floor" >&2; exit 1
fi
log "done; log appended to $LOG"
