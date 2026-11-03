# Helpers for scripts/e2e_lifecycle.sh (plan 3.A). Sourced, not run. Bash 3.2 compatible (macOS).

E2E_PIDS=()      # every background process this run started, killed by e2e_cleanup
E2E_NAMES=()

e2e_log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
e2e_die() { e2e_log "FAIL: $*" >&2; exit 1; }

# A free TCP port on 127.0.0.1 (the kernel's pick; released before use, so a tiny race remains).
e2e_free_port() {
  python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}

# A base port p such that p..p+40 are all free (the validator's RPC, PubSub, faucet, gossip and
# dynamic range), searched from a random start so parallel runs rarely meet.
e2e_free_range() {
  python3 -I - <<'PY'
import random, socket
def free(p):
    s = socket.socket()
    try:
        s.bind(("127.0.0.1", p)); return True
    except OSError:
        return False
    finally:
        s.close()
for _ in range(200):
    base = random.randrange(21000, 29000, 50)
    if all(free(p) for p in range(base, base + 41)):
        print(base); break
else:
    raise SystemExit("no free port range")
PY
}

# e2e_start NAME LOGFILE CMD...: runs CMD in the background, remembers its PID.
e2e_start() {
  local name=$1 log=$2
  shift 2
  "$@" >"$log" 2>&1 &
  local pid=$!
  # Out of the job table (no "Terminated" notices); bash still reaps it, so kill -0 sees it go.
  disown "$pid"
  E2E_PIDS+=("$pid")
  E2E_NAMES+=("$name")
  e2e_log "started $name (pid $pid), log $log"
}

e2e_alive() { kill -0 "$1" 2>/dev/null; }

# Stops every started process: SIGTERM, up to 10 s, then SIGKILL. Safe to call twice.
e2e_cleanup() {
  local i pid name t
  for ((i = ${#E2E_PIDS[@]} - 1; i >= 0; i--)); do
    pid=${E2E_PIDS[$i]}
    name=${E2E_NAMES[$i]}
    if e2e_alive "$pid"; then
      kill -TERM "$pid" 2>/dev/null || true
      t=0
      while e2e_alive "$pid" && [ "$t" -lt 100 ]; do
        sleep 0.1
        t=$((t + 1))
      done
      if e2e_alive "$pid"; then
        e2e_log "$name (pid $pid) ignored SIGTERM; killing"
        kill -KILL "$pid" 2>/dev/null || true
        sleep 0.2
      fi
      e2e_log "stopped $name"
    fi
  done
  E2E_PIDS=()
  E2E_NAMES=()
}

# e2e_wait_for SECS WHAT PID CMD...: polls CMD (quietly) every 0.3 s until it succeeds; fails
# when SECS pass or when PID (the process being waited on) has exited.
e2e_wait_for() {
  local secs=$1 what=$2 pid=$3
  shift 3
  local start=$SECONDS
  until "$@" >/dev/null 2>&1; do
    e2e_alive "$pid" || e2e_die "$what: the process exited (see its log)"
    [ $((SECONDS - start)) -lt "$secs" ] || e2e_die "$what: not ready within ${secs} s"
    sleep 0.3
  done
  e2e_log "$what ready after $((SECONDS - start)) s"
}

e2e_keygen() { solana-keygen new --no-bip39-passphrase --silent --force -o "$1" >/dev/null; }

# Refuses anything that is not localnet + the mock (L3-1): no Pascal trade host, no remote RPC.
e2e_guard_env() {
  local v
  for v in PASCAL_TRADE_URL PASCAL_DATA_URL PASCAL_WS_URL SOLANA_RPC_URL; do
    case "${!v:-}" in
      *pascal.trade*|*mainnet*) e2e_die "$v=${!v} is set in the environment; the e2e run is localnet + mock only" ;;
    esac
  done
}
