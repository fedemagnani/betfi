#!/usr/bin/env bash
# Plan 3.C: the app on devnet, end to end, `demo` profile.
#
#   just demo-devnet               (serve until Ctrl-C: open the printed app URL)
#   just demo-devnet --e2e         (the browser-only run, then the live-desk phone gate, then stop)
#   just demo-devnet --live        (Pascal's games to come and live books; docs/demo-live.md)
#   just demo                      (scripts/demo.sh: live with a replay fallback, logs streamed)
#   scripts/demo_devnet.sh [--e2e] [--skip-build] [--localnet] [--live] [--fallback]
#
# --fallback (with --live): when no game Pascal lists is left to start (NBA moneylines plus EPL,
# Bundesliga, LaLiga and Champions League matches; any day; DEMO_SLATE_DATE narrows it to one US
# night), stop the live
# mock and play the 28 Oct tape instead, with a notice. The live
# mock starts before the app build for that (its clock is the wall clock).
# DEMO_STREAM=1: stream the mock's, the desk's and the static server's logs to the terminal,
# prefixed [mock] / [desk] / [app] (colour on a TTY, NO_COLOR honoured), RPC and Pascal URLs
# redacted; filters DEMO_MOCK_LOG and DEMO_DESK_LOG (RUST_LOG syntax, default info), and the
# evidence of --e2e goes to the temp dir unless DEMO_OUT is set. The banner lists the app, dev
# wallet and mock URLs and a resolve command per game on the slate.
# DEMO_OPEN=1 (set by `just demo`): open the app URL in the default browser after the banner
# (`open` on macOS, `xdg-open` elsewhere); never with --e2e.
# MOCK_AMM_PROGRAM_ID (5.0a): the desk's /tokens and /swap-quote read the mock AMM's devnet
# pools (`crank amm-setup`); --localnet loads mock-amm too and runs amm-setup.
#
# Starts, all on 127.0.0.1 and all stopped by the trap on every exit path:
#   pascal-mock  the demo tape at --speed 60, permit verification on, a throwaway hedger trading
#                key registered (--trading-key); no script: legs resolve through /mock/resolve.
#                --live (4.L): `pascal-mock --live` instead, the wall clock and Pascal's real
#                catalogue, books and results read from data.pascal.trade (reads only; every
#                order still fills in the mock); DEMO_RECORD=<new dir> also records it as a tape
#   desk         PROFILE=demo on Solana devnet (public RPC), crank = quote signer = the user's
#                crank-authority key (path from .env), throwaway hedger owner / trading key /
#                builder; PASCAL_DATA_URL and PASCAL_TRADE_URL both the local mock (mode M, D-143)
#   the app      built with SCHEDINA_DESK_URL (the desk), SCHEDINA_RPC_URL (public devnet),
#                SCHEDINA_DEV_TOOLS (the in-page burner wallet) and SCHEDINA_SLATE_DATE (the
#                tape's night; with --live, empty unless DEMO_SLATE_DATE is set: every game
#                not over, whatever its day, 2 Nov), served on http://localhost
#                (a secure context)
#
# --e2e then runs app/scripts/devnet_e2e.mjs (headless Chromium, 390 px, light): a fresh burner
# wallet funded with DEMO_FUND_SOL (default 0.012, D-154; at most 0.05) from demo-3.json → faucet
# → two tickets → leg 1 resolves → cash-out → settle, screenshots and every signature in
# $DEMO_OUT; then app/scripts/phone_gate.mjs in live mode against the running desk; then `crank
# status`. Both browser scripts play any slate of 3 or more games (5.0b): with --live on a smaller
# slate, the card and a quote are checked over HTTP instead (no wallet, no SOL spent).
# E2E_TIP, E2E_COPY (+ FUND2_SOL, TIPSTER_SHARE_BPS), E2E_BONK (+ BONK_AMOUNT),
# E2E_SOL and E2E_SOCCER are passed to devnet_e2e.mjs when set; DEMO_FUND_SOL + FUND2_SOL stay
# within 0.05 SOL. E2E_SOL (7.2a-g: a ticket paid in mockSOL) first pegs the mockSOL pool to the
# desk's real SOL price: `crank amm-peg --from-fx --desk <this desk> --yes`, signed by the mock
# AMM's admin (the upgrade authority, AUTHORITY_KEYPAIR_PATH in .env; never given to the desk),
# up to DEMO_PEG_RUNS (default 8) bounded runs, until one sends nothing (within 1%). The peg's
# swaps are paid by the authority, not by demo-3.
#
# Devnet and localhost only (l3-contracts L3-1): no request to a Pascal trade host, mainnet
# refused by URL and genesis hash. No key material is printed, copied or written to the repo:
# the devnet keys are referenced by path, the hedger keys live in a temp dir removed at exit.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=scripts/e2e/lib.sh
source "$ROOT/scripts/e2e/lib.sh"

E2E=0
SKIP_BUILD=0
CLUSTER=devnet
LIVE=0
FALLBACK=0
for a in "$@"; do
  case "$a" in
    --e2e) E2E=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    --localnet) CLUSTER=localnet ;;
    --live) LIVE=1 ;;
    --fallback) FALLBACK=1 ;;
    -h|--help) sed -n 2,46p "$0"; exit 0 ;;
    *) e2e_die "unknown flag $a (--e2e, --skip-build, --localnet, --live, --fallback)" ;;
  esac
done
[ "$FALLBACK" = 0 ] || [ "$LIVE" = 1 ] || e2e_die "--fallback goes with --live"
STREAM=${DEMO_STREAM:-0}

PROGRAM_ID=HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW
# The mock AMM (4.D, D-200), deployed on devnet by 5.0a; /tokens offers mockBONK where its pools exist.
MOCK_AMM_PROGRAM_ID=${MOCK_AMM_PROGRAM_ID:-5WhsDY2XjzJoWDbe1AgJnnNgJccQwte9TgsvG4yRtHbf}
CRANK_PUBKEY=131Hmjktmt3zTFt7MEDs4eq76fFdw1cnBk81tGqu1CGp # Config.crank = Config.quote_signer
DEVNET_GENESIS=EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG
# Public devnet by default. D-131 (29 Oct, approved): a hosted devnet RPC from SOLANA_RPC_URL
# (environment or .env) serves the desk and the crank; see below.
RPC=https://api.devnet.solana.com
WS=wss://api.devnet.solana.com
if [ "$CLUSTER" = localnet ]; then
  # The app derives the WebSocket port from :8899 (app/src/config.rs), so the default ports.
  RPC=http://127.0.0.1:8899
  WS=ws://127.0.0.1:8900
fi
SO=${SCHEDINA_SO:-$ROOT/programs/target/deploy-devnet/schedina.so}
AMM_SO=${MOCK_AMM_SO:-$ROOT/programs/target/deploy-devnet/mock_amm.so}
SIMD_0500=B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g # no new SBPF v0-v2 deployments (D-006)
TAPE=${DEMO_TAPE:-$ROOT/tapes/nba-2026-10-28}
BIN=$ROOT/target/debug
APP=$ROOT/app
PW=${PLAYWRIGHT_CORE:-$ROOT/node_modules/playwright-core/index.mjs}
# The children run with a temp HOME, so the browser path is resolved here.
CHROME_PATH=${CHROME_PATH:-$HOME/Library/Caches/ms-playwright/chromium_headless_shell-1208/chrome-headless-shell-mac-arm64/chrome-headless-shell}
DEMO_OUT_SET=${DEMO_OUT:-}
DEMO_OUT=${DEMO_OUT:-$ROOT/docs/demo-devnet}
FUND_SOL=${DEMO_FUND_SOL:-0.012} # D-154: a full run needs ~0.0085 SOL on the burner
# The mockBONK ticket adds its ATA, a FaucetClaim and a third Pending: 0.012 falls short (localnet
# run of 29 Oct: "this wallet lacks SOL"), 0.02 passes.
[[ ! "${E2E_BONK:-}" =~ ^(1|true|yes)$ ]] || FUND_SOL=${DEMO_FUND_SOL:-0.02}
# The mockSOL ticket (E2E_SOL, 7.2a-g) adds a mockSOL ATA, its FaucetClaim and one more Pending.
[[ ! "${E2E_SOL:-}" =~ ^(1|true|yes)$ ]] || FUND_SOL=${DEMO_FUND_SOL:-0.025}
PEG_RUNS=${DEMO_PEG_RUNS:-8}
case "$PEG_RUNS" in '' | *[!0-9]* | 0) e2e_die "DEMO_PEG_RUNS must be a positive number" ;; esac
DEMO_RECORD=${DEMO_RECORD:-}
if [ -n "$DEMO_RECORD" ]; then
  [ "$LIVE" = 1 ] || e2e_die "DEMO_RECORD records a --live session; add --live"
  case "$DEMO_RECORD" in /*) ;; *) DEMO_RECORD=$PWD/$DEMO_RECORD ;; esac
  [ ! -e "$DEMO_RECORD/raw/books.jsonl" ] || e2e_die "$DEMO_RECORD already holds a recording"
fi

e2e_guard_env
for tool in solana solana-keygen python3 curl node dx cargo; do
  command -v "$tool" >/dev/null || e2e_die "$tool is not on PATH"
done
awk -v s="$FUND_SOL" 'BEGIN { exit !(s > 0 && s <= 0.05) }' ||
  e2e_die "DEMO_FUND_SOL=$FUND_SOL: the burner gets at most 0.05 SOL"
# devnet_e2e.mjs's optional L5 parts (5.0b), passed through its clean environment when set: an own
# tip (E2E_TIP), its copy from a second burner (E2E_COPY, funded with FUND2_SOL, default 0.008),
# a ticket paid in mockBONK (E2E_BONK), one paid in mockSOL (E2E_SOL, 7.2a-g), soccer on the
# slate or not (E2E_SOCCER), and their amounts. Both burners together get at most 0.05 SOL from
# demo-3 (L5-3).
E2E_PASS=()
for v in E2E_TIP E2E_COPY E2E_BONK E2E_SOL E2E_SOCCER FUND2_SOL BONK_AMOUNT TIPSTER_SHARE_BPS; do
  [ -z "${!v:-}" ] || E2E_PASS+=("$v=${!v}")
done
if [[ "${E2E_COPY:-}" =~ ^(1|true|yes)$ ]]; then
  awk -v a="$FUND_SOL" -v b="${FUND2_SOL:-0.008}" 'BEGIN { exit !(b > 0 && a + b <= 0.05) }' ||
    e2e_die "DEMO_FUND_SOL + FUND2_SOL = $FUND_SOL + ${FUND2_SOL:-0.008}: both burners get at most 0.05 SOL"
fi

TMP=$(mktemp -d "${TMPDIR:-/tmp}/schedina-demo.XXXXXX")
KEYS=$TMP/keys
mkdir -p "$KEYS" "$TMP/home"
chmod 700 "$KEYS"

# --- keys: devnet paths from the gitignored .env, parsed (never sourced, D-134); localnet keys
# are all generated in the temp dir (L3-2), so the devnet signers never touch a localnet run.
if [ "$CLUSTER" = localnet ]; then
  for k in authority crank funder sponsor; do e2e_keygen "$KEYS/$k.json"; done
  CRANK_KEY=$KEYS/crank.json
  QS_KEY=$CRANK_KEY
  FUNDER_KEY=$KEYS/funder.json
  SPONSOR_KEY=$KEYS/sponsor.json
  CRANK_PUBKEY=$(solana-keygen pubkey "$CRANK_KEY")
  DEMO_OUT=${DEMO_OUT_SET:-$TMP/out}
fi
# A live run's evidence (and `just demo`'s) stays in the temp dir unless DEMO_OUT says otherwise.
[ "$LIVE" != 1 ] && [ "$STREAM" != 1 ] || DEMO_OUT=${DEMO_OUT_SET:-$TMP/out}
ENV_FILE=${SCHEDINA_ENV_FILE:-$ROOT/.env}
env_path() {
  local var=$1 line val
  if [ -n "${!var:-}" ]; then printf '%s' "${!var}"; return; fi
  [ -f "$ENV_FILE" ] || return 0
  line=$(grep -E "^[[:space:]]*(export[[:space:]]+)?$var=" "$ENV_FILE" | tail -n 1) || return 0
  val=${line#*=}
  val=${val%\"}; val=${val#\"}; val=${val%\'}; val=${val#\'}
  case "$val" in "~/"*) val="$HOME/${val#\~/}" ;; "\$HOME/"*) val="$HOME/${val#\$HOME/}" ;; esac
  printf '%s' "$val"
}

# D-131: a hosted devnet RPC (SOLANA_RPC_URL in the environment or .env) for the desk and the
# crank. Its URL carries an API key: it is never logged (only *_SHOWN, query string cut), and it
# reaches the app bundle only with DEMO_APP_HOSTED_RPC=1 (localhost builds; never publish that
# build). The app otherwise keeps the public endpoint.
DESK_RPC=$RPC; DESK_WS=$WS
if [ "$CLUSTER" = devnet ]; then
  HOSTED=$(env_path SOLANA_RPC_URL)
  if [ -n "$HOSTED" ]; then
    DESK_RPC=$HOSTED
    case "$HOSTED" in
      https://*) DESK_WS="wss://${HOSTED#https://}" ;;
      http://*) DESK_WS="ws://${HOSTED#http://}" ;;
    esac
    # SOLANA_WS_URL (environment or .env) overrides the derived socket: a hosted RPC may refuse
    # `logsSubscribe` (Helius devnet answers -32603), which leaves the desk resyncing every ~30 s.
    WS_SET=$(env_path SOLANA_WS_URL)
    [ -z "$WS_SET" ] || DESK_WS=$WS_SET
    if [ "${DEMO_APP_HOSTED_RPC:-0}" = 1 ]; then
      RPC=$DESK_RPC; WS=$DESK_WS
      e2e_log "WARNING: the app bundle embeds the hosted RPC URL (API key); serve it on localhost only"
    fi
  fi
fi
shown() { local u=${1%%\?*}; [ "$u" = "$1" ] && printf '%s' "$u" || printf '%s?…' "$u"; }
RPC_SHOWN=$(shown "$RPC"); DESK_RPC_SHOWN=$(shown "$DESK_RPC")
if [ "$CLUSTER" = devnet ]; then
  KEYS_DIR=$(env_path SCHEDINA_KEYS_DIR)
  KEYS_DIR=${KEYS_DIR:-$HOME/.config/schedina}
  CRANK_KEY=$(env_path CRANK_KEYPAIR_PATH)
  CRANK_KEY=${CRANK_KEY:-$KEYS_DIR/crank.json}
  QS_KEY=$(env_path QUOTE_SIGNER_KEYPAIR_PATH)
  QS_KEY=${QS_KEY:-$CRANK_KEY}
  FUNDER_KEY=${DEMO_FUNDER_KEYPAIR:-$KEYS_DIR/demo-3.json}
  # L8: the relay's fee and rent payer; a missing file leaves sponsoring off (self-paid sends).
  SPONSOR_KEY=$(env_path SPONSOR_KEYPAIR_PATH)
  SPONSOR_KEY=${SPONSOR_KEY:-$KEYS_DIR/sponsor.json}
fi
for k in "$CRANK_KEY" "$QS_KEY"; do
  [ -f "$k" ] || e2e_die "missing key file $k (set SCHEDINA_ENV_FILE to the main checkout's .env)"
done
# Public keys only; the files are never read by anything but solana-keygen and the desk.
[ "$(solana-keygen pubkey "$CRANK_KEY")" = "$CRANK_PUBKEY" ] ||
  e2e_die "CRANK_KEYPAIR_PATH is not Config.crank $CRANK_PUBKEY"
[ "$(solana-keygen pubkey "$QS_KEY")" = "$CRANK_PUBKEY" ] ||
  e2e_die "QUOTE_SIGNER_KEYPAIR_PATH is not Config.quote_signer $CRANK_PUBKEY"
if [ "$E2E" = 1 ]; then
  [ -f "$FUNDER_KEY" ] || e2e_die "missing the funder key $FUNDER_KEY (demo-3.json)"
  [ -f "$PW" ] || e2e_die "playwright-core not found at $PW (set PLAYWRIGHT_CORE)"
  [ -x "$CHROME_PATH" ] || e2e_die "no headless Chromium at $CHROME_PATH (set CHROME_PATH)"
fi

# --- devnet, by genesis hash (refuses mainnet and anything else) --------------------------------
if [ "$CLUSTER" = devnet ]; then
  for url in "$RPC" "$DESK_RPC"; do
    GENESIS=$(curl -fsS --max-time 20 "$url" -H 'content-type: application/json' \
      -d '{"jsonrpc":"2.0","id":1,"method":"getGenesisHash"}' |
      python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])') ||
      e2e_die "cannot read the genesis hash from $(shown "$url")"
    [ "$GENESIS" = "$DEVNET_GENESIS" ] || e2e_die "$(shown "$url") is not devnet (genesis $GENESIS)"
  done
fi

# The tape's night (US Eastern date of its first tip-off) for the Card. Live, no date (2 Nov):
# the Card holds every game Pascal lists that is not over, whatever its day, as Pascal's page does;
# DEMO_SLATE_DATE pins one US night.
tape_date() {
  python3 -I - "$TAPE/manifest.json" <<'PY'
import datetime, json, sys
games = json.load(open(sys.argv[1]))["games"]
first = min(g["start_ms"] for g in games)
print(datetime.datetime.fromtimestamp(first / 1000 - 5 * 3600, datetime.timezone.utc).date())
PY
}
if [ "$LIVE" = 1 ]; then
  SLATE_DATE=${DEMO_SLATE_DATE:-}
else
  SLATE_DATE=${DEMO_SLATE_DATE:-$(tape_date)}
fi

OK=0
on_exit() {
  local code=$?
  # Ignore a second Ctrl-C (or HUP) while stopping: it would kill us before the rm below.
  trap - EXIT
  trap '' INT TERM HUP
  e2e_cleanup
  if [ "${DEMO_KEEP:-0}" = 1 ] || [ "$OK" != 1 ]; then
    rm -rf "$KEYS" # the throwaway hedger keys never outlive the run
    e2e_log "logs kept in $TMP"
  else
    rm -rf "$TMP"
  fi
  if [ "$OK" = 1 ]; then e2e_log "DEMO OK"; else e2e_log "DEMO STOPPED (exit $code)"; fi
  exit "$code"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# --- throwaway Pascal keys (mode M: the mock is the only venue that sees them) -----------------
for k in hedger-owner hedger-trading builder; do e2e_keygen "$KEYS/$k.json"; done
HEDGER_PK=$(solana-keygen pubkey "$KEYS/hedger-owner.json")
TRADING_PK=$(solana-keygen pubkey "$KEYS/hedger-trading.json")
BUILDER_PK=$(solana-keygen pubkey "$KEYS/builder.json")

# --- ports (the desk URL is baked into the app at build time) ----------------------------------
MOCK_PORT=$(e2e_free_port)
DESK_PORT=$(e2e_free_port)
APP_PORT=$(e2e_free_port)
MOCK=http://127.0.0.1:$MOCK_PORT
DESK=http://127.0.0.1:$DESK_PORT
APP_URL=http://localhost:$APP_PORT

# --- DEMO_STREAM: the services' logs on the terminal (L5-2) ------------------------------------
# One python3 process follows mock.log, desk.log and app.log (by path, so a replaced file is
# picked up), prints each line once with its [name] prefix, formats the desk's JSON lines, drops
# per-frame noise, and redacts the RPC URLs (they carry an API key) and Pascal URLs. It is a
# started process like the services, so the exit trap stops it (last, after a final drain), and
# it also exits on its own when this script's PID goes away.
start_streamer() {
  local color=0
  [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && color=1
  # By default, hide the hedge desk's per-leg warnings about earlier runs' devnet tickets (each
  # run starts with an empty desk DB and the indexer replays them); DEMO_LOG_QUIET= shows all.
  local quiet='hedge: (minted ticket unknown to the desk and the DB|leg resolved on a ticket the desk does not know)'
  cat >"$TMP/stream.py" <<'PY'
import json, os, re, signal, sys, time

color = os.environ.get("STREAM_COLOR") == "1"
secrets = sorted({s for s in os.environ.get("STREAM_SECRETS", "").split("\n") if len(s) > 8}, key=len, reverse=True)
parent = int(os.environ["STREAM_PARENT"])
srcs = list(zip(sys.argv[1::2], sys.argv[2::2]))
PREFIX = {"mock": "36", "desk": "35", "app": "32"}
LEVEL = {"ERROR": "31", "WARN": "33"}
API_KEY = re.compile(r"([?&]api[-_]?key=)[^\s\"&)]*", re.I)
PASCAL = re.compile(r"(?:https?|wss?)://[a-z0-9.-]*pascal\.trade[^\s\"',)]*", re.I)
MOCK_LINE = re.compile(r"^\d{4}-\d\d-\d\dT(\d\d:\d\d:\d\d)\.\d+Z\s+(\w+)\s+(?:pascal_mock::)?(.*)$")
# Per-frame or per-poll lines that drown the story (a filter of DEMO_*_LOG can still show them).
NOISE = re.compile(os.environ.get("STREAM_QUIET") or r"(?!)")
BUS = os.environ.get("STREAM_BUS") == "1"
FEED_TEXT = re.compile(r'text: "((?:[^"\\]|\\.)*)"')
WIDTH = int(os.environ.get("STREAM_WIDTH") or 0)

def redact(s):
    for x in secrets:
        s = s.replace(x, "<rpc>")
    return PASCAL.sub("<pascal>", API_KEY.sub(r"\1…", s))

def fmt(name, line):
    level = ""
    if name == "desk" and line.startswith("{"):
        try:
            j = json.loads(line)
        except ValueError:
            return level, line
        f = dict(j.get("fields") or {})
        msg = f.pop("message", "")
        target = (j.get("target") or "").removeprefix("desk::")
        level = (j.get("level") or "").upper()
        if target == "state" and msg == "bus" and not BUS:
            # The bus echoes every event as a Debug dump; the feed's sentence says it best.
            t = FEED_TEXT.search(str(f.get("detail", ""))) if f.get("event") == "feed" else None
            if not t:
                return level, None
            f = {}
            target, msg = "feed", t.group(1)
        kv = " ".join(f"{k}={v}" for k, v in f.items())
        return level, f'{(j.get("timestamp") or "")[11:19]} {level:<5} {target}: {msg} {kv}'.rstrip()
    m = MOCK_LINE.match(line)
    if m:
        level = m.group(2).upper()
        return level, f"{m.group(1)} {level:<5} {m.group(3)}"
    return level, line

def emit(name, line):
    line = line.rstrip("\r")
    if not line.strip():
        return
    level, text = fmt(name, redact(line))
    if text is None or NOISE.search(text):
        return
    if WIDTH and len(text) > WIDTH:
        text = text[: WIDTH - 1] + "…"
    if color:
        tag = f"\033[1;{PREFIX.get(name, '37')}m[{name}]\033[0m"
        if level in LEVEL:
            text = f"\033[{LEVEL[level]}m{text}\033[0m"
    else:
        tag = f"[{name}]"
    sys.stdout.write(f"{tag} {text}\n")

state = {}
def poll():
    for name, path in srcs:
        try:
            st = os.stat(path)
        except OSError:
            continue
        cur = state.get(name)
        if cur is None or os.fstat(cur[0].fileno()).st_ino != st.st_ino or st.st_size < cur[0].tell():
            if cur:
                cur[0].close()
            cur = state[name] = [open(path, "r", errors="replace"), ""]
        data = cur[0].read()
        if data:
            lines = (cur[1] + data).split("\n")
            cur[1] = lines.pop()
            for l in lines:
                emit(name, l)
    sys.stdout.flush()

stopping = []
signal.signal(signal.SIGTERM, lambda *_: stopping.append(1))
signal.signal(signal.SIGINT, signal.SIG_IGN)  # Ctrl-C: the script's trap stops us after a drain
while not stopping:
    try:
        os.kill(parent, 0)
    except OSError:
        break
    poll()
    time.sleep(0.25)
poll()  # the services' last lines (they are stopped before us)
PY
  STREAM_COLOR=$color STREAM_PARENT=$$ \
    STREAM_SECRETS="$(printf '%s\n' "$DESK_RPC" "$DESK_WS" "$RPC" "$WS" "$(shown "$DESK_RPC")" "$(shown "$DESK_WS")")" \
    STREAM_QUIET="${DEMO_LOG_QUIET-$quiet}" STREAM_BUS="${DEMO_LOG_BUS:-0}" STREAM_WIDTH="${DEMO_LOG_WIDTH:-240}" \
    python3 -I "$TMP/stream.py" mock "$TMP/mock.log" desk "$TMP/desk.log" app "$TMP/app.log" &
  local pid=$!
  disown "$pid"
  E2E_PIDS+=("$pid")
  E2E_NAMES+=(logs)
  e2e_log "streaming the mock's, the desk's and the app server's logs (pid $pid)"
}

# The start-up block: where to click, and how to end each game on the mock.
banner() {
  local mode
  if [ "$LIVE" = 1 ]; then mode="LIVE: Pascal's card and books through the mock ($SLATE_GAMES games to come)"
  elif [ -n "$FELL_BACK" ]; then mode="REPLAY (fallback): the 28 Oct 2026 night at 60x"
  else mode="REPLAY: the 28 Oct 2026 night at 60x"; fi
  python3 -I - "$DESK" "$MOCK" "$APP_URL" "$SLATE_DATE" "$CLUSTER" "$mode" "$STREAM" <<'PY' || e2e_log "banner: cannot read the card"
import datetime, json, sys, urllib.request
desk, mock, app, date, cluster, mode, stream = sys.argv[1:8]
def get(url):
    with urllib.request.urlopen(url, timeout=20) as r:
        return json.load(r)
card = get(f"{desk}/card?sport=nba&date={date}")
try:
    tokens = ", ".join(t["symbol"] for t in get(f"{desk}/tokens")["tokens"])
except Exception as e:
    tokens = f"(unavailable: {e})"
bar = "=" * 96
print(bar)
print(f" SCHEDINA DEMO · {cluster} · {mode}")
print(f"   app          {app}")
print(f"   dev wallet   {app}/dev/wallet   (in-page burner wallet)")
print(f"   mock         {mock}   ({mock}/mock/info)")
print(f"   desk         {desk}   (/health, /card, /tokens)")
print(f"   stake in     {tokens}")
clock = "" if mode.startswith("LIVE") else "; tip-offs in tape time, which runs at 60x"
slate = f"Slate {date} (US Eastern{clock})" if date else "Every game Pascal lists that is not over"
print(f" {slate}.")
if card.get("games"):
    print(" End an NBA game on the mock (1 = away team wins, 0 = home team wins):")
for g in card.get("games", []):
    start = datetime.datetime.fromtimestamp(g["start_ms"] / 1000, datetime.timezone.utc)
    print(f'   {g["away"]["name"]} @ {g["home"]["name"]}, {start:%a %d %b %H:%M} UTC')
    body = json.dumps({"symbol": g["symbol"], "price": "1"})
    print(f"     curl -s -X POST {mock}/mock/resolve -H 'content-type: application/json' -d '{body}'")
try:
    soccer = get(f"{desk}/card?sport=soccer&date={date}").get("games", [])
except Exception:
    soccer = []
if soccer:
    print(" Soccer: end a match by resolving its winning outcome to 1 (the home win shown; the draw")
    print(" and away win are the same symbol with .DRAW or the away code after the dot):")
for g in soccer:
    start = datetime.datetime.fromtimestamp(g["start_ms"] / 1000, datetime.timezone.utc)
    print(f'   {g["home"]["name"]} v {g["away"]["name"]} ({g.get("league", "soccer")}), {start:%a %d %b %H:%M} UTC')
    body = json.dumps({"symbol": g["symbol"], "price": "1"})
    print(f"     curl -s -X POST {mock}/mock/resolve -H 'content-type: application/json' -d '{body}'")
print(" Ctrl-C stops everything." + (" Logs follow, prefixed [mock] [desk] [app]." if stream == "1" else ""))
print(bar)
PY
}

# --- build ------------------------------------------------------------------------------------
if [ "$SKIP_BUILD" != 1 ]; then
  e2e_log "building desk, crank, pascal-mock"
  (cd "$ROOT" && cargo build --locked -p desk --bins -p pascal-mock) >"$TMP/build.log" 2>&1 ||
    e2e_die "cargo build failed: $TMP/build.log"
fi
for b in desk crank pascal-mock; do [ -x "$BIN/$b" ] || e2e_die "$BIN/$b missing"; done

# A clean environment for every child: nothing from the caller's shell leaks in.
CLEAN_ENV=(env -i "PATH=$PATH" "HOME=$TMP/home" "TMPDIR=${TMPDIR:-/tmp}")

# --- Pascal mock ------------------------------------------------------------------------------
# Replay: started after the build, since at 60x the first tip-off is about 4 min of wall clock
# away and the desk refuses legs that have started (venue clock). Live: the wall clock, Pascal's
# catalogue (loaded before the mock serves, up to 45 s) and books, orders filled in the mock;
# started before the app build, so a small slate can still fall back to the tape (--fallback).
start_mock() {
  e2e_start mock "$TMP/mock.log" "${CLEAN_ENV[@]}" RUST_LOG="${DEMO_MOCK_LOG:-info}" NO_COLOR=1 \
    "$BIN/pascal-mock" "$@" --bind "127.0.0.1:$MOCK_PORT" --trading-key "$HEDGER_PK=$TRADING_PK"
  MOCK_PID=${E2E_PIDS[${#E2E_PIDS[@]} - 1]}
}
# Polls CMD quietly until it succeeds; non-zero when SECS pass or PID exits (no die).
wait_quiet() {
  local secs=$1 pid=$2 start=$SECONDS
  shift 2
  until "$@" >/dev/null 2>&1; do
    e2e_alive "$pid" && [ $((SECONDS - start)) -lt "$secs" ] || return 1
    sleep 0.3
  done
}
# Stops one started process and drops it from E2E_PIDS (so the exit trap never signals a
# recycled PID).
stop_started() {
  local pid=$1 i t=0 pids=() names=()
  kill -TERM "$pid" 2>/dev/null || true
  while e2e_alive "$pid" && [ "$t" -lt 50 ]; do sleep 0.1; t=$((t + 1)); done
  e2e_alive "$pid" && kill -KILL "$pid" 2>/dev/null || true
  for i in "${!E2E_PIDS[@]}"; do
    [ "${E2E_PIDS[i]}" = "$pid" ] || { pids+=("${E2E_PIDS[i]}"); names+=("${E2E_NAMES[i]}"); }
  done
  E2E_PIDS=(${pids[@]+"${pids[@]}"}); E2E_NAMES=(${names[@]+"${names[@]}"})
}
# The games that have not started (3 Nov: soccer too, the leagues the desk lists), from the
# mock's copy of Pascal's catalogue: one per NBA moneyline, one per soccer match (its 1-X-2).
count_slate() {
  curl -fsS --max-time 20 "$MOCK/api/v1/markets" | python3 -I -c '
import json, sys, time
date, now = sys.argv[1], time.time() * 1000
games = set()
for m in json.load(sys.stdin).get("data", []):
    d = m.get("display_attributes", {})
    mt = d.get("market_type") or {}
    gid = (d.get("game_page") or {}).get("game_id", "")
    start = int(d.get("expected_event_start_time_ms") or 0)
    moneyline = (mt.get("market") or {}).get("type") == "moneyline"
    if not (moneyline and gid.endswith(date) and start > now):
        continue
    nba = mt.get("sport") == "basketball" and "NBA" in d.get("tags", [])
    soccer = mt.get("sport") == "soccer" and m.get("symbol", "").split("_")[0] in ("EPL", "BUND", "LALIGA", "UCL")
    if nba or soccer:
        games.add(gid)
print(len(games))' "$SLATE_DATE"
}

[ "$STREAM" != 1 ] || start_streamer
SLATE_GAMES=
FELL_BACK=
if [ "$LIVE" = 1 ]; then
  MOCK_ARGS=(--live)
  [ -z "$DEMO_RECORD" ] || MOCK_ARGS+=(--record "$DEMO_RECORD")
  start_mock "${MOCK_ARGS[@]}"
  why=
  if ! wait_quiet 60 "$MOCK_PID" curl -fsS "$MOCK/api/v1/time"; then
    why="the live mock did not come up within 60 s (Pascal's read API unreachable?)"
  elif ! SLATE_GAMES=$(count_slate); then
    why="the live catalogue could not be read from the mock"; SLATE_GAMES=
  else
    e2e_log "live: $(curl -fsS "$MOCK/mock/info") · $SLATE_GAMES games (NBA and soccer) ${SLATE_DATE:+on $SLATE_DATE }not started yet"
    # Live whenever one game is left to play (3 Nov, user request: the replay only without one).
    if [ "$SLATE_GAMES" -eq 0 ]; then
      why="no game (NBA or soccer) ${SLATE_DATE:+on $SLATE_DATE }is left to play on Pascal"
    fi
  fi
  if [ -n "$why" ]; then
    if [ "$FALLBACK" != 1 ] || [ -n "$DEMO_RECORD" ]; then
      e2e_die "$why: run without --live (replay), see docs/demo-live.md"
    fi
    stop_started "$MOCK_PID"
    mv "$TMP/mock.log" "$TMP/mock-live.log"
    LIVE=0; SLATE_GAMES=; FELL_BACK=$why
    SLATE_DATE=${DEMO_SLATE_DATE_REPLAY:-$(tape_date)}
    e2e_log "NOTICE ==========================================================================="
    e2e_log "NOTICE: $why."
    e2e_log "NOTICE: falling back to the REPLAY of the 28 Oct 2026 night (tapes/nba-2026-10-28 at 60x)."
    e2e_log "NOTICE: odds and results come from the tape; run 'just demo' again on a game night for live odds."
    e2e_log "NOTICE ==========================================================================="
  fi
fi

e2e_log "building the app (desk $DESK, app RPC $RPC_SHOWN, desk RPC $DESK_RPC_SHOWN, slate ${SLATE_DATE:-every game not over}, dev tools on)"
(cd "$APP" && env SCHEDINA_DESK_URL="$DESK" SCHEDINA_RPC_URL="$RPC" SCHEDINA_DEV_TOOLS=1 \
  SCHEDINA_SLATE_DATE="$SLATE_DATE" dx build --platform web --release) >"$TMP/dx-build.log" 2>&1 ||
  e2e_die "dx build failed: $TMP/dx-build.log"
SITE=$APP/target/dx/schedina-app/release/web/public
[ -f "$SITE/index.html" ] || e2e_die "no $SITE/index.html"

# --- localnet rehearsal: a fresh validator with the devnet-feature program, init and funding -----
crank_cli() { "${CLEAN_ENV[@]}" "$BIN/crank" --rpc "$DESK_RPC" --program-id "$PROGRAM_ID" "$@"; }
if [ "$CLUSTER" = localnet ]; then
  command -v solana-test-validator >/dev/null || e2e_die "solana-test-validator is not on PATH"
  [ -f "$SO" ] || e2e_die "$SO missing (just e2e builds it, or set SCHEDINA_SO)"
  [ -f "$AMM_SO" ] || e2e_die "$AMM_SO missing (just swap-chain-test builds it, or set MOCK_AMM_SO)"
  for port in 8899 8900; do
    ! lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1 || e2e_die "port $port is taken"
  done
  AUTH_PK=$(solana-keygen pubkey "$KEYS/authority.json")
  e2e_start validator "$TMP/validator.log" "${CLEAN_ENV[@]}" solana-test-validator \
    --ledger "$TMP/ledger" --quiet --bind-address 127.0.0.1 --rpc-port 8899 \
    --faucet-port 9900 --dynamic-port-range 18000-18040 \
    --upgradeable-program "$PROGRAM_ID" "$SO" "$AUTH_PK" \
    --upgradeable-program "$MOCK_AMM_PROGRAM_ID" "$AMM_SO" "$AUTH_PK" --deactivate-feature "$SIMD_0500"
  VALIDATOR_PID=${E2E_PIDS[${#E2E_PIDS[@]} - 1]}
  height_ok() { [ "$(solana -u "$RPC" block-height 2>/dev/null || echo 0)" -gt 1 ]; }
  e2e_wait_for 90 validator "$VALIDATOR_PID" height_ok
  for pk in "$AUTH_PK" "$CRANK_PUBKEY" "$(solana-keygen pubkey "$FUNDER_KEY")" "$(solana-keygen pubkey "$SPONSOR_KEY")"; do
    solana -u "$RPC" airdrop -k "$KEYS/authority.json" 100 "$pk" >/dev/null || e2e_die "airdrop to $pk"
  done
  # Devnet's parameters (docs/deploy.md).
  crank_cli --authority "$KEYS/authority.json" init-config --crank "$CRANK_PUBKEY" \
    --quote-signer "$CRANK_PUBKEY" --max-legs 5 --max-payout-usdc 500 --min-stake-usdc 5 \
    --reserve-usdc 1000 >"$TMP/init.log"
  # One faucet claim (hourly per owner) covers the vault (5,000) and amm-setup's two pools (2,000 each).
  crank_cli --authority "$KEYS/authority.json" faucet --to "$AUTH_PK" --usdc 9000 >/dev/null
  crank_cli --authority "$KEYS/authority.json" fund-vault --usdc 5000 >/dev/null
  e2e_log "localnet: program $PROGRAM_ID initialised and funded on $RPC"
  crank_cli --authority "$KEYS/authority.json" amm-setup --amm-program-id "$MOCK_AMM_PROGRAM_ID" \
    >"$TMP/amm-setup.log" 2>&1 || e2e_die "amm-setup failed: $TMP/amm-setup.log"
  # Idempotent: a second pass must send nothing.
  crank_cli --authority "$KEYS/authority.json" amm-setup --amm-program-id "$MOCK_AMM_PROGRAM_ID" \
    >>"$TMP/amm-setup.log" 2>&1 || e2e_die "amm-setup (re-run) failed: $TMP/amm-setup.log"
  [ "$(grep -c '^amm-setup: nothing sent' "$TMP/amm-setup.log")" = 1 ] ||
    e2e_die "amm-setup re-run sent transactions: $TMP/amm-setup.log"
  e2e_log "localnet: mock AMM $MOCK_AMM_PROGRAM_ID set up ($(grep -c '^tx ' "$TMP/amm-setup.log") transactions; re-run sent nothing)"
fi

# --- Pascal mock, replay (the live one is already up) -------------------------------------------
if [ "$LIVE" != 1 ]; then
  start_mock --tape "$TAPE" --speed 60 --no-script
  e2e_wait_for 30 mock "$MOCK_PID" curl -fsS "$MOCK/api/v1/time"
fi

# --- desk -------------------------------------------------------------------------------------
# The upgrade authority is never given to the desk: AUTHORITY_KEYPAIR_PATH points at a file that
# does not exist. Polls are spaced for the public RPC (the desk retries 429 politely).
e2e_start desk "$TMP/desk.log" "${CLEAN_ENV[@]}" RUST_LOG="${DEMO_DESK_LOG:-info}" \
  PROFILE=demo DESK_LISTEN="127.0.0.1:$DESK_PORT" DESK_DB="$TMP/desk.sqlite" \
  PASCAL_DATA_URL="$MOCK" PASCAL_TRADE_URL="$MOCK" \
  PASCAL_HEDGER_OWNER="$HEDGER_PK" PASCAL_HEDGER_TRADING_KEY_PATH="$KEYS/hedger-trading.json" \
  PASCAL_BUILDER_PUBKEY="$BUILDER_PK" \
  SOLANA_RPC_URL="$DESK_RPC" SOLANA_WS_URL="$DESK_WS" PROGRAM_ID="$PROGRAM_ID" SCHEDINA_KEYS_DIR="$KEYS" \
  MOCK_AMM_PROGRAM_ID="$MOCK_AMM_PROGRAM_ID" \
  CRANK_KEYPAIR_PATH="$CRANK_KEY" QUOTE_SIGNER_KEYPAIR_PATH="$QS_KEY" \
  SPONSOR_KEYPAIR_PATH="$SPONSOR_KEY" AUTHORITY_KEYPAIR_PATH="$KEYS/no-authority.json" \
  CRANK_RESOLVE_POLL_S=2 CRANK_RESOLVED_SWEEP_S=20 CRANK_RESOLVE_AFTER_S=0 CRANK_TICK_MS=1000 \
  INDEXER_RESYNC_S=30 HEDGE_ROLL_TIMEOUT_MS=3000 \
  "$BIN/desk"
DESK_PID=${E2E_PIDS[${#E2E_PIDS[@]} - 1]}
e2e_wait_for 60 desk "$DESK_PID" curl -fsS "$DESK/health"
e2e_log "relay: $(curl -sS "$DESK/relay")"
if [ "$LIVE" = 1 ]; then
  # Pascal's whole list takes the desk's catalogue a moment: /card answers 503 until then.
  desk_catalogue() { curl -fsS "$DESK/health" | grep -q '"catalogue":{'; }
  e2e_wait_for 60 "desk catalogue pass" "$DESK_PID" desk_catalogue
fi
# Any game of any sport, as the app asks (3 Nov: a night can be soccer only).
card_ready() { curl -fsS "$DESK/card?date=$SLATE_DATE" | grep -q '"symbol"'; }
e2e_wait_for 60 "desk catalogue" "$DESK_PID" card_ready
PROFILE_SEEN=$(curl -fsS "$DESK/health" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["profile"])')
[ "$PROFILE_SEEN" = demo ] || e2e_die "the desk runs profile $PROFILE_SEEN, expected demo"

# --- the app ----------------------------------------------------------------------------------
e2e_start app "$TMP/app.log" "${CLEAN_ENV[@]}" node "$APP/scripts/serve.mjs" "$SITE" "$APP_PORT"
APP_PID=${E2E_PIDS[${#E2E_PIDS[@]} - 1]}
e2e_wait_for 20 app "$APP_PID" curl -fsS "http://127.0.0.1:$APP_PORT/"
e2e_log "mock $MOCK · desk $DESK (profile demo, $CLUSTER) · app $APP_URL"
e2e_log "logs: $TMP/{mock,desk,app}.log"
banner

if [ "$E2E" != 1 ] && [ "${DEMO_OPEN:-0}" = 1 ]; then
  if command -v open >/dev/null 2>&1; then open "$APP_URL" || true
  elif command -v xdg-open >/dev/null 2>&1; then xdg-open "$APP_URL" >/dev/null 2>&1 || true
  else e2e_log "no browser opener (open, xdg-open): open $APP_URL yourself"; fi
fi

if [ "$E2E" != 1 ]; then
  # Ctrl-C (the whole process group gets SIGINT) or SIGTERM is the normal way out here.
  OK=1
  trap 'exit 0' INT TERM
  while e2e_alive "$MOCK_PID" && e2e_alive "$DESK_PID" && e2e_alive "$APP_PID"; do
    sleep 1 || true
  done
  sleep 0.5 || true # a Ctrl-C reaches the services too: let the trap win the race
  OK=0
  e2e_die "a service exited (logs in $TMP)"
fi

# --- E2E_SOL (7.2a-g): the mockSOL pool pegged to the real SOL price --------------------------
# The app pays €x in mockSOL at the desk's /fx SOL price (V-3); the pool must pay that much back.
# `amm-peg` moves at most 10% of a reserve per run, so it runs until a run sends nothing (within
# 1%). Signed by AmmConfig.admin (the upgrade authority on devnet, the generated authority on
# localnet), a key this script never gives to the desk.
if [ "$E2E" = 1 ] && [[ "${E2E_SOL:-}" =~ ^(1|true|yes)$ ]]; then
  if [ "$CLUSTER" = localnet ]; then
    PEG_KEY=$KEYS/authority.json
  else
    PEG_KEY=$(env_path AUTHORITY_KEYPAIR_PATH)
    PEG_KEY=${PEG_KEY:-$KEYS_DIR/upgrade-authority.json}
  fi
  [ -f "$PEG_KEY" ] || e2e_die "E2E_SOL: missing the mock AMM admin key $PEG_KEY (set AUTHORITY_KEYPAIR_PATH)"
  # The desk reads the SOL price every 60 s; until its first answer /fx serves the fallback.
  sol_priced() { curl -fsS "$DESK/fx" | grep -q '"sol_fallback":false'; }
  PEG_START=$SECONDS
  until sol_priced || [ $((SECONDS - PEG_START)) -ge 90 ]; do sleep 2; done
  sol_priced || e2e_log "NOTE: the desk still serves its fixed SOL fallback after 90 s; pegging to it (DESK_SOL_URL?)"
  PEGGED=0
  for i in $(seq 1 "$PEG_RUNS"); do
    crank_cli --authority "$PEG_KEY" amm-peg --from-fx --desk "$DESK" --amm-program-id "$MOCK_AMM_PROGRAM_ID" --yes \
      >"$TMP/amm-peg-$i.log" 2>&1 || e2e_die "amm-peg run $i failed: $TMP/amm-peg-$i.log"
    { grep -E '^ *(target|reserves)|^amm-peg: (1 transaction sent|within|nothing)' "$TMP/amm-peg-$i.log" || true; } |
      while IFS= read -r l; do e2e_log "amm-peg run $i: ${l#"${l%%[![:space:]]*}"}"; done
    if grep -q '^amm-peg: within 1% of the target; nothing sent' "$TMP/amm-peg-$i.log"; then
      PEGGED=1
      break
    fi
  done
  [ "$PEGGED" = 1 ] ||
    e2e_die "the mockSOL pool is not within 1% after $PEG_RUNS amm-peg runs (DEMO_PEG_RUNS; logs $TMP/amm-peg-*.log); no burner funded"
  e2e_log "mockSOL pool pegged to the desk's SOL price ($i amm-peg run(s); the last sent nothing)"
fi

# --- the live-desk phone gate, then the browser-only run --------------------------------------
# The gate first: it needs 3 or more games on the card, unstarted (no wallet, quotes only).
mkdir -p "$DEMO_OUT" "$TMP/gate"
FAILS=()
set +e
if [ "$LIVE" = 1 ] && [ "$SLATE_GAMES" -lt 3 ]; then
  # Both browser scripts need a slate of 3 or more games (app/scripts/slate.mjs, 5.0b). On a
  # smaller live slate the card and a quote are checked over HTTP instead: no wallet, no SOL.
  e2e_log "live: $SLATE_GAMES games ${SLATE_DATE:+on $SLATE_DATE }to come; the browser runs need 3: checking the card and a quote over HTTP (no SOL spent)"
  echo '[]' >"$TMP/phone-gate.json"
  python3 -I - "$DESK" "$MOCK" "$SLATE_DATE" <<'PY' 2>&1 | tee "$TMP/devnet-e2e.log"
import datetime, json, sys, urllib.request
desk, mock, date = sys.argv[1:4]
def get(url):
    with urllib.request.urlopen(url, timeout=20) as r:
        return json.load(r)
def post(url, body):
    req = urllib.request.Request(url, json.dumps(body).encode(), {"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)
card = get(f"{desk}/card?sport=nba&date={date}")
games = card["games"]
assert games, "no game on the card"
for g in games:
    start = datetime.datetime.fromtimestamp(g["start_ms"] / 1000, datetime.timezone.utc)
    odds = " / ".join(f'{p["pick"]} {p["odds_u"] / 1e6:.2f}' for p in g["picks"])
    print(f'card {g["away"]["name"]} @ {g["home"]["name"]} {start:%H:%M} UTC: {odds}')
    assert len(g["picks"]) == 2 and all(p["odds_u"] > 0 for p in g["picks"]), g["symbol"]
legs = [{"symbol": g["symbol"], "side": "long"} for g in games[:3]]
q = post(f"{desk}/quote", {"legs": legs, "stake_u": 5_000_000, "token_in": None, "copied_from": None})
syms = ",".join(l["symbol"] for l in legs)
books = get(f"{mock}/api/v1/books?symbols={syms}")["data"]["books"]
for leg in q["legs"]:
    ask = int(round(float(books[leg["symbol"]]["asks"][0][0]) * 1e6))
    print(f'quote leg {leg["pick"]} price {leg["price_u"] / 1e6:.3f} (live best ask {ask / 1e6:.3f})')
    assert ask <= leg["price_u"] <= ask + 50_000, (leg, ask)
info = get(f"{mock}/mock/info")
assert info["mode"] == "live" and info["upstream_ok"] and info["stale_books"] == 0, info
print(f'LIVE CHECK PASS: {len(games)} games on {date} with live odds; {len(legs)}-leg quote at '
      f'{q["odds_offered_u"] / 1e6:.2f} pays {q["payout_u"] / 1e6:.2f} USDC on 5; mock {json.dumps(info)}')
PY
  [ "${PIPESTATUS[0]}" = 0 ] || FAILS+=("the live card and quote check failed")
else
  "${CLEAN_ENV[@]}" PLAYWRIGHT_CORE="$PW" CHROME_PATH="$CHROME_PATH" GATE_MODE=live \
    node "$APP/scripts/phone_gate.mjs" "$APP_URL" "$TMP/gate" >"$TMP/phone-gate.json" 2>&1
  [ "$?" = 0 ] || FAILS+=("phone_gate.mjs (live) failed: $TMP/phone-gate.json")
  "${CLEAN_ENV[@]}" PLAYWRIGHT_CORE="$PW" CHROME_PATH="$CHROME_PATH" \
    APP_URL="$APP_URL" DESK_URL="$DESK" MOCK_URL="$MOCK" RPC_URL="$RPC" \
    FUNDER_KEYPAIR="$FUNDER_KEY" FUND_SOL="$FUND_SOL" OUT="$DEMO_OUT" CRANK_BIN="$BIN/crank" \
    CRANK_PUBKEY="$CRANK_PUBKEY" CLUSTER="$CLUSTER" MOCK_AMM_PROGRAM_ID="$MOCK_AMM_PROGRAM_ID" \
    SLATE_DATE="$SLATE_DATE" ${E2E_PASS[@]+"${E2E_PASS[@]}"} \
    node "$APP/scripts/devnet_e2e.mjs" 2>&1 | tee "$TMP/devnet-e2e.log"
  [ "${PIPESTATUS[0]}" = 0 ] || FAILS+=("devnet_e2e.mjs failed")
fi
set -e
# The gate's file holds its JSON report and then its summary line (app/scripts/report.mjs).
"${CLEAN_ENV[@]}" node "$APP/scripts/report.mjs" gate "$TMP/phone-gate.json" || true

echo
crank_cli status 2>&1 | tee "$TMP/status-end.log" || true

# --- log checks -------------------------------------------------------------------------------
if grep -E -i -n 'pascal\.trade' "$TMP/desk.log" | grep -v -i 'data\.pascal\.trade' | head -5 | grep .; then
  FAILS+=("the desk log names a Pascal trade host")
fi
DESK_ERRORS=$(grep -n '"level":"ERROR"' "$TMP/desk.log" || true)
if [ "$LIVE" = 1 ]; then
  # Earlier devnet tickets are on markets of past nights, absent from today's catalogue: the
  # indexer keeps their leg hashes and says so. Not a failure of this run.
  OLD=$(printf '%s\n' "$DESK_ERRORS" | grep -c 'cannot name the legs of a ticket' || true)
  [ "$OLD" = 0 ] || e2e_log "note: $OLD earlier devnet tickets are on past markets (legs kept as hashes)"
  DESK_ERRORS=$(printf '%s\n' "$DESK_ERRORS" | grep -v 'cannot name the legs of a ticket' || true)
fi
if printf '%s\n' "$DESK_ERRORS" | cut -c1-300 | head -10 | grep .; then
  FAILS+=("the desk logged errors")
fi
if [ "$LIVE" = 1 ]; then
  # Live: an upstream reconnect is a warning, not a failure; errors still fail the run.
  grep -n -E ' WARN ' "$TMP/mock.log" | cut -c1-300 | head -5 | sed 's/^/note: mock /' || true
  if grep -n -E ' ERROR ' "$TMP/mock.log" | cut -c1-300 | head -10 | grep .; then
    FAILS+=("the mock logged errors")
  fi
  { curl -fsS "$MOCK/mock/info"; echo; } | sed 's/^/live: /' || true
elif grep -n -E ' (ERROR|WARN) ' "$TMP/mock.log" | cut -c1-300 | head -10 | grep .; then
  FAILS+=("the mock logged warnings or errors")
fi
N429=$(grep -c -E 'HTTP 429|Too Many Requests' "$TMP/desk.log" || true)
[ "$N429" = 0 ] || e2e_log "note: $N429 rate-limited RPC answers in the desk log (retried)"
for pid in "$MOCK_PID" "$DESK_PID" "$APP_PID"; do
  e2e_alive "$pid" || FAILS+=("a service (pid $pid) died during the run")
done
if [ "${#FAILS[@]}" -gt 0 ]; then
  for f in "${FAILS[@]}"; do e2e_log "FAIL: $f"; done
  e2e_die "${#FAILS[@]} check(s) failed; logs in $TMP"
fi
e2e_log "summary: $(grep -E '^(DEVNET E2E|LIVE CHECK) PASS' "$TMP/devnet-e2e.log" || echo '?')"
OK=1
