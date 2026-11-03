# Shared by scripts/deploy.sh (plan steps 0.B, 3.B) and later scripts.
# Source it after setting CLUSTER (localnet | devnet) and optionally RPC_URL.
#
# Keys live outside the repo, in ~/.config/schedina/ (override SCHEDINA_KEYS_DIR). Never rely on
# the Solana CLI's default signer or its configured URL: this machine's CLI config points at
# mainnet, so every solana call below and in the scripts passes --url and -k explicitly.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "${CLUSTER:-}" in
  localnet|devnet) ;;
  *) echo "--cluster must be localnet or devnet" >&2; exit 2 ;;
esac

# Devnet only: key paths (and SOLANA_RPC_URL) not set in the environment are read from the
# gitignored .env ($SCHEDINA_ENV_FILE, default the checkout's .env; from a worktree point it at
# the main checkout's). Lines are parsed as VAR=value, never executed. Localnet never reads it,
# so the devnet signers cannot leak into a localnet run (l3-contracts L3-2).
ENV_FILE="${SCHEDINA_ENV_FILE:-$ROOT/.env}"
env_from_file() {
  local var="$1" line val
  [ -n "${!var:-}" ] && return 0
  [ -f "$ENV_FILE" ] || return 0
  line=$(grep -E "^[[:space:]]*(export[[:space:]]+)?$var=" "$ENV_FILE" | tail -n 1) || return 0
  val="${line#*=}"
  val="${val%\"}"; val="${val#\"}"; val="${val%\'}"; val="${val#\'}"
  case "$val" in
    "~/"*) val="$HOME/${val#\~/}" ;;
    "\$HOME/"*) val="$HOME/${val#\$HOME/}" ;;
  esac
  [ -n "$val" ] && export "$var=$val"
  return 0
}
if [ "$CLUSTER" = devnet ]; then
  for _v in SCHEDINA_KEYS_DIR AUTHORITY_KEYPAIR_PATH CRANK_KEYPAIR_PATH QUOTE_SIGNER_KEYPAIR_PATH \
            PROGRAM_KEYPAIR MOCK_AMM_PROGRAM_KEYPAIR SOLANA_RPC_URL SPONSOR_KEYPAIR_PATH; do
    env_from_file "$_v"
  done
  unset _v
fi

KEYS_DIR="${SCHEDINA_KEYS_DIR:-$HOME/.config/schedina}"

# Program keypair, gitignored; it only exists in the main checkout's keys/ (not in worktrees):
# override with PROGRAM_KEYPAIR. The program ID is read from it, never hard-coded here.
PROGRAM_KEYPAIR="${PROGRAM_KEYPAIR:-$ROOT/keys/schedina-program-keypair.json}"
# The mock AMM's program keypair (5.0a, D-200), same rule; read only by deploy.sh --program mock-amm.
MOCK_AMM_PROGRAM_KEYPAIR="${MOCK_AMM_PROGRAM_KEYPAIR:-$ROOT/keys/mock-amm-program-keypair.json}"
# Program authority: upgrade authority (deploy fee payer, signs `solana program deploy`) and
# Config.authority (init_config, set_params, pause, withdraw_house). Same file as Anchor.toml's
# provider wallet.
AUTHORITY_KEYPAIR="${AUTHORITY_KEYPAIR_PATH:-$KEYS_DIR/upgrade-authority.json}"
# Backend keys stored in Config: the crank (resolve_leg, finalize, settle) and the quote signer
# (ed25519 key open_pending verifies quotes against).
CRANK_KEYPAIR="${CRANK_KEYPAIR_PATH:-$KEYS_DIR/crank.json}"
QUOTE_SIGNER_KEYPAIR="${QUOTE_SIGNER_KEYPAIR_PATH:-$KEYS_DIR/quote-signer.json}"
# The desk's sponsor (L8): pays fees and rent of the transactions POST /relay co-signs. Never the
# crank, the quote signer or the authority (the desk refuses to sponsor with any of them).
SPONSOR_KEYPAIR="${SPONSOR_KEYPAIR_PATH:-$KEYS_DIR/sponsor.json}"
# Demo wallets (players), funded from the mock-USDC faucet in step 3.B.
DEMO_KEYPAIRS=("$KEYS_DIR/demo-1.json" "$KEYS_DIR/demo-2.json" "$KEYS_DIR/demo-3.json")

# Anchor workspace and the `--features devnet` build, kept apart from `anchor build`'s
# target/deploy/ that the LiteSVM tests load.
PROGRAMS_DIR="$ROOT/programs"
PROGRAM_MANIFEST="$PROGRAMS_DIR/schedina/Cargo.toml"
SO_DIR="$PROGRAMS_DIR/target/deploy-devnet"
SO="$SO_DIR/schedina.so"

case "$CLUSTER" in
  localnet) RPC_URL="${RPC_URL:-http://127.0.0.1:8899}" ;;
  # SOLANA_RPC_URL from the environment (e.g. a hosted devnet endpoint), else the public one.
  devnet) RPC_URL="${RPC_URL:-${SOLANA_RPC_URL:-https://api.devnet.solana.com}}" ;;
esac
# What logs may show: hosted RPCs carry the API key in the query string (`?api-key=…`), which
# must never reach a log, a file or a commit. Print $RPC_SHOWN, never $RPC_URL.
RPC_SHOWN="${RPC_URL%%\?*}"
[ "$RPC_SHOWN" = "$RPC_URL" ] || RPC_SHOWN="$RPC_SHOWN?…"
_rpc_lc=$(printf '%s' "$RPC_URL" | tr '[:upper:]' '[:lower:]')
case "$_rpc_lc" in
  *mainnet*|m|mainnet-beta) echo "refusing a mainnet RPC URL: $RPC_SHOWN" >&2; exit 2 ;;
esac
unset _rpc_lc
# The URL check above misses aliased endpoints; the genesis hash does not. --cluster devnet must
# also really be devnet (a stray SOLANA_RPC_URL could point it at localnet).
GENESIS="$(solana genesis-hash --url "$RPC_URL" -k "$AUTHORITY_KEYPAIR" 2>/dev/null || true)"
if [ "$GENESIS" = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d" ]; then
  echo "refusing $RPC_SHOWN: it is mainnet-beta" >&2; exit 2
fi
if [ "$CLUSTER" = devnet ] && [ "$GENESIS" != "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG" ]; then
  echo "$RPC_SHOWN is not devnet (genesis hash '${GENESIS:-unreachable}')" >&2; exit 2
fi

log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }

need_file() {
  [ -f "$1" ] || { echo "missing $2: $1" >&2; exit 1; }
}

pubkey_of() { solana-keygen pubkey "$1"; }

# Paths for logs and docs: $HOME shown as ~.
tilde() { case "$1" in "$HOME"/*) printf '~%s' "${1#"$HOME"}" ;; *) printf '%s' "$1" ;; esac; }

# Filter for anything that may echo the RPC URL (CLI errors, reqwest messages): the full URL
# becomes $RPC_SHOWN and any api-key query value is cut.
redact_stream() {
  RPC_FULL="$RPC_URL" RPC_SHOW="$RPC_SHOWN" awk '{
    s = $0; f = ENVIRON["RPC_FULL"]; r = ENVIRON["RPC_SHOW"]; out = ""
    if (f != r) { while ((i = index(s, f)) > 0) { out = out substr(s, 1, i - 1) r; s = substr(s, i + length(f)) } }
    s = out s
    gsub(/[?&]api[-_]?key=[^ "&)]*/, "?…", s)
    print s; fflush()
  }'
}

# Program ID from the keypair file; a PROGRAM_ID in the environment must agree with it.
need_file "$PROGRAM_KEYPAIR" "program keypair (set PROGRAM_KEYPAIR)"
KEYPAIR_PROGRAM_ID="$(pubkey_of "$PROGRAM_KEYPAIR")"
if [ -n "${PROGRAM_ID:-}" ] && [ "$PROGRAM_ID" != "$KEYPAIR_PROGRAM_ID" ]; then
  echo "PROGRAM_ID=$PROGRAM_ID but $PROGRAM_KEYPAIR is $KEYPAIR_PROGRAM_ID" >&2; exit 1
fi
PROGRAM_ID="$KEYPAIR_PROGRAM_ID"
export SOLANA_RPC_URL="$RPC_URL" PROGRAM_ID

# SOL balance in lamports.
lamports_of() {
  solana balance --url "$RPC_URL" -k "$AUTHORITY_KEYPAIR" --lamports "$1" | awk '{print $1}'
}

# Rent-exempt minimum in lamports for an account of $1 data bytes, read from the cluster.
rent_of() {
  solana rent --url "$RPC_URL" -k "$AUTHORITY_KEYPAIR" --lamports "$1" | awk '/Rent-exempt minimum/ {print $3}'
}

sol_of() { awk -v l="$1" 'BEGIN { printf "%.9f", l / 1e9 }'; }

# Localnet: airdrop until $1 holds at least $2 SOL. Devnet airdrops are rate-limited, so on
# devnet this only reports a shortfall (fund the address yourself).
ensure_sol() {
  local addr="$1" want_sol="$2" have
  have=$(lamports_of "$addr")
  if [ "$have" -ge $((want_sol * 1000000000)) ]; then return 0; fi
  if [ "$CLUSTER" = localnet ]; then
    solana airdrop --url "$RPC_URL" -k "$AUTHORITY_KEYPAIR" "$want_sol" "$addr" >/dev/null
    log "airdropped $want_sol SOL to $addr"
  else
    echo "$addr holds $((have / 1000000)) milli-SOL on devnet; needs $want_sol SOL. Fund it, then re-run." >&2
    return 1
  fi
}

# Exact-lamport variant: $1 must hold $2 lamports (plus $3 lamports it can count on, e.g. a
# resumable buffer it will get back), for $4 (what for). Localnet airdrops the difference; devnet
# stops with the exact shortfall.
ensure_lamports() {
  local addr="$1" need="$2" extra="${3:-0}" what="${4:-this run}" have short
  have=$(( $(lamports_of "$addr") + extra ))
  if [ "$have" -ge "$need" ]; then return 0; fi
  short=$((need - have))
  if [ "$CLUSTER" = localnet ]; then
    solana airdrop --url "$RPC_URL" -k "$AUTHORITY_KEYPAIR" "$(sol_of $((short + 1000000000)))" "$addr" >/dev/null
    log "airdropped $(sol_of $((short + 1000000000))) SOL to $addr"
    return 0
  fi
  echo "$addr has $(sol_of "$have") SOL usable on $CLUSTER; $what needs $(sol_of "$need") SOL:" \
       "short by $short lamports ($(sol_of "$short") SOL). Fund it, then re-run." >&2
  return 1
}

# Program-derived address of $PROGRAM_ID for seeds given in solana-CLI form (string:config ...).
pda() {
  solana find-program-derived-address --url "$RPC_URL" -k "$AUTHORITY_KEYPAIR" --output json \
    "$PROGRAM_ID" "$@" | jq -r .address
}

# Classic SPL-Token associated token account of owner $1 for mint $2.
ata_of() {
  solana find-program-derived-address --url "$RPC_URL" -k "$AUTHORITY_KEYPAIR" --output json \
    ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL \
    "pubkey:$1" pubkey:TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA "pubkey:$2" | jq -r .address
}

# Raw token amount (base units) of SPL token account $1; 0 when the account does not exist.
# Any other RPC failure is fatal, so a flaky read never triggers a second faucet or deposit.
token_amount() {
  local out err rc=0
  err=$(mktemp)
  out=$(solana account --url "$RPC_URL" -k "$AUTHORITY_KEYPAIR" --output json "$1" 2>"$err") || rc=$?
  if [ "$rc" -ne 0 ]; then
    if grep -q "AccountNotFound\|Unable to find" "$err"; then rm -f "$err"; echo 0; return 0; fi
    redact_stream < "$err" >&2; rm -f "$err"
    echo "token_amount: cannot read $1" >&2; return 1
  fi
  rm -f "$err"
  printf '%s' "$out" | python3 -I -c '
import base64, json, sys
a = json.load(sys.stdin)["account"]
if a["owner"] != "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA":
    sys.exit("not an SPL token account")
d = base64.b64decode(a["data"][0])
if len(d) != 165:
    sys.exit("not an SPL token account")
print(int.from_bytes(d[64:72], "little"))'
}
