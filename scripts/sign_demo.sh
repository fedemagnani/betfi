#!/usr/bin/env bash
# Signed-not-sent evidence (plan step 3.D', mode M, decision D-007 amendment).
#
# Generates a throwaway hedger trading key, owner and builder in a temp dir, runs the desk's own
# hedge desk (crates/desk/tests/live_guard.rs, test `sign_demo`) on the inception hedge of a
# 3-leg NBA moneyline schedina against an in-process pascal-mock on 127.0.0.1 (recorded book of
# 28 Oct 2026, every permit signature verified), and prints:
#
#   1. the exact JSON body the desk POSTed to the mock's /api/v1/orders, which is the body it
#      would POST to Pascal's /api/v1/orders with PASCAL_TRADE_URL set (same code, same bytes);
#   2. `pascal-cli verify-order` on that body against the throwaway trading key, with every
#      permit field and its byte offsets (builder bytes 120..152, fee rate 184..188);
#   3. the mock's fills of the batch, with the builder fee each would pay.
#
# Nothing is sent to Pascal: the only network peer is the mock on 127.0.0.1. The key seeds never
# leave the temp dir, which is removed on exit.
#
# Usage: scripts/sign_demo.sh [OUT_DIR]   OUT_DIR keeps batch.json, signed_order.log, fills.txt,
#                                         verify.txt and pubkeys.txt (no secret key material).
set -euo pipefail

cd "$(dirname "$0")/.."
keep="${1:-}"

work="$(mktemp -d "${TMPDIR:-/tmp}/schedina-sign-demo.XXXXXX")"
trap 'rm -rf "$work"' EXIT
umask 077

# 32 random bytes as 64 hex characters: a seed pascal-rs reads as a key file.
seed() { od -An -tx1 -N32 /dev/urandom | tr -d ' \n'; }
seed >"$work/trading.key"
seed >"$work/owner.key"
seed >"$work/builder.key"

echo "building pascal-cli and the desk's live_guard test (first run takes a while)..." >&2
cli() { cargo run --locked -q -p pascal-rs --bin pascal-cli -- "$@"; }

trading="$(cli pubkey "$work/trading.key")"
owner="$(cli pubkey "$work/owner.key")"
builder="$(cli pubkey "$work/builder.key")"

out="$work/out"
SIGN_DEMO_OUT="$out" \
  PASCAL_HEDGER_OWNER="$owner" \
  PASCAL_HEDGER_TRADING_KEY_PATH="$work/trading.key" \
  PASCAL_BUILDER_PUBKEY="$builder" \
  cargo test --locked -q -p desk --test live_guard sign_demo -- --ignored --exact >"$work/test.log" 2>&1 ||
  {
    cat "$work/test.log" >&2
    exit 1
  }

if command -v python3 >/dev/null; then
  pretty() { python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin), indent=2))'; }
else
  pretty() { cat; }
fi

{
  echo "# Throwaway keys (generated now, never registered on Pascal; seeds deleted on exit)"
  echo "PASCAL_HEDGER_OWNER=$owner"
  echo "trading key (PASCAL_HEDGER_TRADING_KEY_PATH) pubkey: $trading"
  echo "PASCAL_BUILDER_PUBKEY=$builder"
} >"$out/pubkeys.txt"

cli verify-order "$out/batch.json" --signer "$trading" --builder "$builder" \
  --builder-fee-rate 0.001 >"$out/verify.txt"

cat "$out/pubkeys.txt"
echo
echo "# The POST /api/v1/orders body, exactly as the desk sent it (one line, $(wc -c <"$out/batch.json" | tr -d ' ') bytes):"
cat "$out/batch.json"
echo
echo
echo "# The same body, indented:"
pretty <"$out/batch.json"
echo
echo "# pascal-cli verify-order batch.json --signer $trading --builder $builder --builder-fee-rate 0.001"
cat "$out/verify.txt"
echo
echo "# The desk's quote and the mock's fills of this batch (venue mock, never Pascal)"
cat "$out/fills.txt"
echo
echo "# The desk's log, target schedina::signed_order: $(wc -l <"$out/signed_order.log" | tr -d ' ') lines"

if [ -n "$keep" ]; then
  mkdir -p "$keep"
  cp "$out"/{batch.json,signed_order.log,fills.txt,verify.txt,pubkeys.txt} "$keep/"
  echo "kept in $keep" >&2
fi
