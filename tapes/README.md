# Tapes

A **tape** is a recording of real Pascal read-API responses for a handful of markets,
taken from before tip-off until every market resolves. The mock (`pascal-mock`,
spec section 9, `replay` mode) serves `/books`, `/trades`, `/markets/search` and
`/markets` from a tape, time-scaled, so the desk and the app run against real books
without real games. Tapes are recorded by `scripts/record_tape.py` (stdlib only), or by the
mock's live mode (`pascal-mock --live --record <dir>`, below).

## Layout

```
tapes/<name>/
  manifest.json            selection and summary (committed)
  markets_selected.json    GET /api/v1/markets at selection, `data` filtered to the tape's symbols (committed)
  sample/                  trimmed tape, about 1 MB, for tests (committed)
    books.jsonl trades.jsonl search.jsonl markets.jsonl manifest.json
  raw/                     full recording (gitignored: tapes/*/raw/)
    books.jsonl            GET  /api/v1/books?symbols=<symbols>              every 5 s (30 s until 2 h before the first tip-off)
    trades.jsonl           GET  /api/v1/trades?symbols=<symbols>&limit=50    every 30 s
    search.jsonl           POST /api/v1/markets/search {"symbols":[…]}       every 60 s from first tip-off + 2 h, every 10 min before
    markets.jsonl          GET  /api/v1/markets, hourly, `data` filtered to the tape's symbols
    markets_full.json.gz   the full /markets body at selection (about 7 MB uncompressed)
    recorder.log           one line per request (REQ/BACKOFF/ERR/RESOLVED/STOP)
    recorder.pid, nohup.out
```

### Line format

Every `.jsonl` line is one HTTP response:

```json
{"recv_ms":1793212846588,"method":"GET","path":"/api/v1/books?symbols=…","http":200,"body":{…}}
```

`recv_ms` is the local receive time (Unix ms). `body` is the response body **verbatim**:
the bytes as served are spliced into the line (Pascal returns compact JSON), so keys,
key order and string numbers are untouched, and `body.seq` / `body.state_time_ms` /
`body.server_time_ms` are the server's. Error envelopes (`status: "error"`) are kept too.
The only non-verbatim lines are in `markets.jsonl`, whose `data` is filtered to the tape's
symbols; those lines carry a `"trimmed"` field saying so.

NBA moneylines on Pascal have **one binary symbol per game** (e.g.
`NBA_26OCT28_PHIIND_ML.PHI`, long = away team, short = home team). A resolved market
has `resolution {price, seq, market_resolution_ts_ms}` in `/markets/search` (`"1.000000"`
if the long side won, `"0.000000"` otherwise), and its `/books` entry becomes
`{"asks":[],"bids":[]}`.

### manifest.json

`symbols`, `game_ids`, `games[]` (symbol, teams, long/short labels, `start_ms`,
`expected_resolution_ms`, CET strings, `overlaps_with`, selection score),
`overlapping_pairs`, `selection_time_ms`, `catalogue` (seq of the /markets used). After the
recorder stops (or after `finalize`): `resolutions` (symbol → price), per game
`resolution`, `books` coverage (first/last `state_time_ms`, non-empty snapshot count),
`books_from_before_tipoff`, `books_after_resolution`; `raw_files` (line counts, bytes,
first/last `state_time_ms` per file); tape-wide `first_state_time_ms` / `last_state_time_ms`;
`recorder.stop_reason`. `sample/manifest.json` adds a `sample` section with the trim rules
and kept line counts.

## Record

```sh
mkdir -p tapes/nba-2026-10-28/raw
nohup python3 -I scripts/record_tape.py --out tapes/nba-2026-10-28 \
    --max-games 5 --books-every 5 --trades-every 30 --search-every 60 --max-hours 14 \
    > tapes/nba-2026-10-28/raw/nohup.out 2>&1 &
echo $! > tapes/nba-2026-10-28/raw/recorder.pid
```

It fetches the catalogue once, picks NBA moneyline games starting within 24 h (the next
slate if none; it then waits), ranks them by resting depth near mid and spreads them over
distinct tip-off times, keeps at least two overlapping, and records until every selected
market carries `resolution` (+120 s) or `--max-hours`. Rate: about 0.25 req/s on average;
requests are serialised at least 0.25 s apart, `/markets` once per hour; 429 and 5xx back off
exponentially with jitter (Retry-After honoured); transport errors are logged, never fatal.
Other knobs: `--warmup-hours/--warmup-every`, `--search-from-hours/--search-early-every`,
`--linger`. Watch it with `tail -f tapes/<name>/raw/recorder.log`.

### Record from the live mock

`pascal-mock --live --record <dir>` (plan 4.L) writes what its live mode receives in this
format: the books the desk asked about, as the upstream `book` socket last described them, every
5 s when they changed; each `/markets` refresh (filtered, `trimmed`); the backstop
`/markets/search` answers and each result pushed on `market_update`. `<dir>` must be new:

```
<dir>/manifest.json  markets_selected.json  raw/{books,markets,search}.jsonl
```

These books lines are built from the socket, not REST responses, and carry
`"source":"ws book"`. They hold the upstream's levels only, never the mock's fills or resting
orders. A book that was stale (socket dropped, snapshot pending) is left out of the snapshots of
that interval, so a replay cannot fill on it either. A pushed result's line carries
`"source":"ws market_update"`. `trades.jsonl` is not written (the live mode does not follow
public trades). `just mock --tape <dir>` replays the session; `record_tape.py trim` is not
needed for a short session. The manifest has `symbols`, `games[]` (with `start_ms`),
`resolutions`, `raw_files` and `recorder.stop_reason` (`SIGINT`, `SIGTERM`, …).

## Stop

```sh
kill $(cat tapes/nba-2026-10-28/raw/recorder.pid)
```

SIGTERM/SIGINT stop it cleanly and finalize `manifest.json`. If it was killed hard
(`kill -9`, reboot), rebuild the summary from the raw files:

```sh
python3 -I scripts/record_tape.py finalize --tape tapes/nba-2026-10-28
```

## Trim

```sh
python3 -I scripts/record_tape.py trim --tape tapes/nba-2026-10-28 \
    --out tapes/nba-2026-10-28/sample --max-mb 1
```

Keeps lines verbatim, never edits them: the first and last `search` line plus every one in
which a symbol's `resolution` first appears; `trades` lines whose list of trade ids changed;
the first `markets` line; then `books` evenly thinned over the whole tape to fill the
budget, always keeping the first and last snapshot and, per symbol, the last non-empty
book and the snapshot after it (the resolution edge).

## Demo

The demo replays `nba-2026-10-28` at 60× and resolves leg 1 (`NBA_26OCT28_PHIIND_ML.PHI`,
PHI wins) 40 s of wall clock after boot, from `script.toml`:

```sh
just mock-demo                       # = just mock --tape tapes/nba-2026-10-28 --speed 60
```

The mock listens on `127.0.0.1:8790`: REST under `/api/v1` (point the desk's
`PASCAL_DATA_URL` at `http://127.0.0.1:8790`) and the WebSocket at `ws://127.0.0.1:8790/ws`
(`account`, `market_update`, `book`). It uses `raw/` when present, else `sample/`. Every
order, cancel and mass cancel is verified with `pascal-rs`'s permit code: a bad signature
is `UNAUTHORIZED` (401). Extra arguments go to the binary:

```sh
# only this trading key may sign for this owner (repeatable; default: any valid signature)
just mock-demo --trading-key <owner>=<trading key>
# mode R: boot the hedger from a saved GET /api/v1/accounts/{owner} response
just mock-demo --account-snapshot <owner>=docs/hedger-account.json
# frozen clock for tests (wall+40s still fires 40 s after boot, at the frozen time)
just mock --tape tapes/nba-2026-10-28/sample --speed 0
```

Timeline: `T+0` the mock boots at the tape's first snapshot (about 4 h before tip-off); the
desk quotes off `/books` and hedges with signed IOCs that fill against the replayed book (the
`account` socket pushes each fill with `builder_fee_paid_usd`); `T+40 s` (40 min of tape)
PHI resolves at 1: `POST /markets/search` and `/markets/resolved` show the resolution, the
`market_update` socket pushes it, and the hedger's PHI position settles. Other resolutions:
`curl -s localhost:8790/mock/resolve -d '{"symbol":"…","price":"0"}'`; clock:
`/mock/clock` (`{"advance_ms":…}` or `{"speed":…}`). Errors on demand: `?inject=WOULD_CROSS`
on any route, or `POST /mock/inject {"code":"RATE_LIMITED","path":"/api/v1/orders"}`.
`cargo test -p pascal-mock --test l2 demo_script_real_time -- --ignored` runs this demo in
real time (about 40 s).
