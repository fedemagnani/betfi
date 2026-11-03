"""Refresh app/fixtures/desk/ from the desk's canonical C4 examples (D-045, D-046).

Usage, from app/:  python3 -I fixtures/gen_fixtures.py ../fixtures/desk fixtures/desk

Every *.json of the desk's fixtures/desk/ is copied verbatim, and the demo stream script
stream.json ([{"delay_ms", "frame"}], replayed by src/stream.rs in demo mode) is assembled from
the desk's stream_*.json frames: a schedina escrowed, hedged, minted and its ticket pushed, then
a second one refunded. Nothing is computed: every number comes from the desk's files.

The L4 routes' examples (tipsters.json, tipster.json, house.json, copy_quote.json, and the swap
routes' tokens.json and swap_quote.json) are taken from the desk's files of the same shape
whatever their names; while the desk has none, the app's own (written from
prompts/l4-contracts.md L4-2 by plan 4.B, L4-3 by 5.0b) stay.
"""
import glob
import json
import os
import shutil
import sys

SRC, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)

# Files of a previous generator that the desk does not have.
for stale in ("quote_1.json", "quote_2.json", "quote_3.json"):
    path = os.path.join(OUT, stale)
    if os.path.exists(path):
        os.remove(path)

# The app's own board and profile, before the desk's files of the same names land (see below).
app_tipsters = {}
for name in ("tipsters.json", "tipster.json"):
    path = os.path.join(OUT, name)
    if os.path.exists(path):
        with open(path, "rb") as f:
            app_tipsters[name] = f.read()

copied = []
for path in sorted(glob.glob(os.path.join(SRC, "*.json"))):
    name = os.path.basename(path)
    shutil.copyfile(path, os.path.join(OUT, name))
    copied.append(name)

# L4 routes (L4-2). The app reads them under fixed names; the desk's examples (4.A) may use
# other names, so each is recognised by its shape and copied, verbatim, under the app's name.
# Until the desk has one, the app keeps its own example written from L4-2 (plan 4.B).
L4_NAMES = {
    "tipsters.json": lambda j: isinstance(j.get("tipsters"), list),
    "tipster.json": lambda j: isinstance(j.get("tipster"), dict) and "history" in j,
    "house.json": lambda j: isinstance(j.get("vault"), dict) and isinstance(j.get("hedger"), dict),
    "copy_quote.json": lambda j: "quote_msg" in j and (j.get("copy_fee_u") or 0) > 0,
    # L4-3 swap routes (written by 5.0b from the contract and fixtures/swap/vectors.json's
    # worked example: 10,000 mockBONK into a 5,000,000 : 5,000 pool).
    "tokens.json": lambda j: isinstance(j.get("tokens"), list),
    "swap_quote.json": lambda j: "min_out_u" in j and "pool" in j and "route" in j,
}
for app_name, is_shape in L4_NAMES.items():
    found = []
    for name in copied:
        with open(os.path.join(SRC, name)) as f:
            try:
                j = json.load(f)
            except ValueError:
                continue
        if isinstance(j, dict) and is_shape(j):
            found.append(name)
    if not found:
        print(f"{app_name}: no desk example yet, keeping the app's")
        continue
    # The exact name wins; otherwise the first match in name order.
    pick = app_name if app_name in found else found[0]
    if len(found) > 1:
        print(f"{app_name}: several desk examples match ({', '.join(found)}), using {pick}")
    if pick != app_name:
        shutil.copyfile(os.path.join(SRC, pick), os.path.join(OUT, app_name))
    print(f"{app_name}: from the desk's {pick}")

# Demo mode opens the board's tipster pages from tipster.json: the desk's pair is used only if
# that profile is one of the board's rows (5.0b: the L4 merge's refresh took the desk's 4.A pair,
# whose profile is not on its board, so the demo's tipster page, its charts and the copy flow
# had nothing to show). Otherwise the app's pair (4.B, from L4-2) is put back.
def load(name):
    with open(os.path.join(OUT, name)) as f:
        return json.load(f)


try:
    board = {t["wallet"] for t in load("tipsters.json")["tipsters"]}
    consistent = load("tipster.json")["tipster"]["wallet"] in board
except (OSError, ValueError, KeyError, TypeError):
    consistent = False
if not consistent and all(n in app_tipsters for n in ("tipsters.json", "tipster.json")):
    for name, body in app_tipsters.items():
        with open(os.path.join(OUT, name), "wb") as f:
            f.write(body)
    print("tipsters.json, tipster.json: the desk's profile is not on its board, keeping the app's")


def frame(name):
    with open(os.path.join(SRC, name)) as f:
        return json.dumps(json.load(f), separators=(",", ":"))


SCRIPT = [
    (300, "stream_heartbeat.json"),
    (1500, "stream_progress_escrowed.json"),
    (1500, "stream_progress_hedging.json"),
    (600, "stream_feed.json"),
    (1500, "stream_progress_minted.json"),
    (200, "stream_ticket.json"),
    (800, "stream_progress_refunded.json"),
]
script = [{"delay_ms": d, "frame": frame(n)} for d, n in SCRIPT]
with open(os.path.join(OUT, "stream.json"), "w") as f:
    json.dump(script, f, indent=1)
    f.write("\n")

print(f"copied {len(copied)} files, wrote stream.json ({len(script)} frames) to {OUT}")
