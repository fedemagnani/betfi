#!/usr/bin/env bash
# The browser gates on local builds (6.2a-g), for `just phone-gate` and `just visual`. No desk, no
# chain: demo and design builds only, served from temp copies on free ports; every server this
# starts is stopped on exit.
#
#   app/scripts/local_gates.sh phone-gate   demo build + design build; the phone gate (demo mode,
#                                           DESIGN_URL = the design build), then the dry run of
#                                           devnet_e2e.mjs's UI path on both (e2e_dry.mjs)
#   app/scripts/local_gates.sh visual       design build; visual/shoot.mjs, its review page and the
#                                           parity page of the eight mockup screens (parity.html,
#                                           parity.md)
#
# Env: PLAYWRIGHT_CORE (default node_modules/playwright-core/index.mjs at the repo root, e.g.
# after `npm i --no-save playwright-core@1.57.0` there), CHROME_PATH, MOCKUP (visual), GATE_OUT (default app/target/phone-gate), VISUAL_OUT
# (default app/scripts/visual/out), SKIP_BUILD=1 (serve the builds of an earlier run, kept in
# app/target/gate-builds/).
set -euo pipefail
cd "$(dirname "$0")/.."
APP=$PWD
export PLAYWRIGHT_CORE=${PLAYWRIGHT_CORE:-$APP/../node_modules/playwright-core/index.mjs}
[ -f "$PLAYWRIGHT_CORE" ] || { echo "playwright-core not found at $PLAYWRIGHT_CORE (set PLAYWRIGHT_CORE)" >&2; exit 2; }
BUILDS=$APP/target/gate-builds
TMP=$(mktemp -d "${TMPDIR:-/tmp}/betfi-gates.XXXXXX")
PIDS=()
cleanup() {
  for p in ${PIDS[@]+"${PIDS[@]}"}; do kill "$p" 2>/dev/null || true; done
  rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

# build <name> [VAR=value…]: a debug web build (dev tools on) with the given env, copied to
# target/gate-builds/<name> (dx writes every build to the same directory).
build() {
  local name=$1; shift
  if [ "${SKIP_BUILD:-}" = 1 ] && [ -f "$BUILDS/$name/index.html" ]; then return; fi
  echo "local gates: dx build ($name)…"
  # Only the given flags: a desk URL or design flag from the caller's shell never leaks in.
  env -u SCHEDINA_DESK_URL -u SCHEDINA_DESIGN_FIXTURES -u SCHEDINA_SLATE_DATE "$@" dx build --platform web >"$TMP/dx-$name.log" 2>&1 || { tail -30 "$TMP/dx-$name.log" >&2; exit 1; }
  rm -rf "${BUILDS:?}/$name" && mkdir -p "$BUILDS" && cp -R target/dx/schedina-app/debug/web/public "$BUILDS/$name"
}

# serve <name>: serves target/gate-builds/<name> on a free port; sets URL. (Not in a command
# substitution: the server's pid must reach PIDS for the exit trap.)
serve() {
  local name=$1 log="$TMP/serve-$1.log" port=""
  node scripts/serve.mjs "$BUILDS/$name" 0 >"$log" 2>&1 &
  PIDS+=("$!")
  for _ in $(seq 1 100); do
    port=$(sed -n 's/^serving .* on \([0-9][0-9]*\)$/\1/p' "$log")
    [ -n "$port" ] && break
    sleep 0.1
  done
  [ -n "$port" ] || { echo "local gates: the $name server did not start: $(cat "$log")" >&2; exit 1; }
  URL="http://127.0.0.1:$port"
}

case "${1:-}" in
  phone-gate)
    build demo SCHEDINA_DESIGN_FIXTURES=0
    build design SCHEDINA_DESIGN_FIXTURES=1
    serve demo; DEMO_URL=$URL
    serve design; DESIGN_URL=$URL
    OUT=${GATE_OUT:-$APP/target/phone-gate}
    rm -rf "$OUT" && mkdir -p "$OUT"
    status=0
    DESIGN_URL="$DESIGN_URL" GATE_MODE=demo node scripts/phone_gate.mjs "$DEMO_URL" "$OUT" >"$OUT/phone-gate.json" 2>&1 || status=1
    node scripts/report.mjs gate "$OUT/phone-gate.json" | grep -v ' ok$' || true
    node scripts/e2e_dry.mjs "$DEMO_URL" "$DESIGN_URL" >"$OUT/e2e-dry.log" 2>&1 || status=1
    grep -v '^ok ' "$OUT/e2e-dry.log" || true
    echo "screenshots and reports: $OUT"
    exit "$status"
    ;;
  visual)
    build design SCHEDINA_DESIGN_FIXTURES=1
    serve design; DESIGN_URL=$URL
    OUT=${VISUAL_OUT:-$APP/scripts/visual/out}
    status=0
    node scripts/visual/shoot.mjs "$DESIGN_URL" "$OUT" >"$TMP/shoot.json" 2>&1 || status=1
    tail -1 "$TMP/shoot.json"
    [ "$status" = 0 ] || grep -B1 -A8 '"ok": false' "$TMP/shoot.json" | head -60 >&2 || true
    echo "review page: $OUT/index.html"
    echo "parity page: $OUT/parity.html (summary: $OUT/parity.md)"
    exit "$status"
    ;;
  *)
    echo "usage: app/scripts/local_gates.sh phone-gate|visual" >&2
    exit 2
    ;;
esac
