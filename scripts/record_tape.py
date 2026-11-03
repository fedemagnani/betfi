#!/usr/bin/env python3
"""Record an NBA "tape" of verbatim Pascal read-API responses, and trim it.

Stdlib only. Run with `python3 -I`.

    python3 -I scripts/record_tape.py --out tapes/nba-2026-10-28 \
        --max-games 5 --books-every 5 --trades-every 30 --search-every 60 --max-hours 14
    python3 -I scripts/record_tape.py trim --tape tapes/nba-2026-10-28 \
        --out tapes/nba-2026-10-28/sample --max-mb 1
    python3 -I scripts/record_tape.py finalize --tape tapes/nba-2026-10-28

record (the default command)
    1. GET /api/v1/markets once (gzip; the full body is kept as raw/markets_full.json.gz).
    2. Select NBA moneyline markets (one binary symbol per game; the reverse outcome
       is the short side of the same symbol) whose games start within the next
       24 h, up to --max-games, at least two overlapping in time. If none start
       within 24 h, the next slate is taken and the recorder waits for it.
    3. Write manifest.json and markets_selected.json (the /markets envelope with
       `data` filtered to the selected symbols).
    4. Loop until every selected market carries `resolution` (plus --linger
       seconds) or --max-hours: GET /books every --books-every s (every
       --warmup-every s while more than --warmup-hours before the first
       tip-off), GET /trades every --trades-every s, POST /markets/search every
       --search-every s from first tip-off + --search-from-hours (every
       --search-early-every s before), GET /markets hourly (filtered).
       Each response is appended to raw/<kind>.jsonl as one line:
           {"recv_ms":…,"method":…,"path":…,"http":…,"body":<response body verbatim>}
       The body bytes are written exactly as served (compact JSON, keys and
       string numbers untouched), so `seq` and `state_time_ms` are the server's.
    5. On exit (resolution, deadline, SIGTERM/SIGINT) manifest.json gets the
       resolution prices, first/last state_time_ms and line counts.

Rate budget (D-015): one request at a time, >= 0.25 s apart (<= 4 req/s);
/markets at most once per 30 s (here: once per hour). 429 and 5xx back off
exponentially with jitter, honouring Retry-After; network errors never crash
the loop.
"""

import argparse
import gzip
import json
import os
import random
import re
import signal
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timedelta, timezone

BASE = "https://data.pascal.trade"
UA = "betfi-schedina-tape/0.1 (+record_tape.py)"
MIN_GAP_S = 0.25
MARKETS_MIN_GAP_S = 30.0
KINDS = ("books", "trades", "search", "markets")

try:
    from zoneinfo import ZoneInfo

    CET = ZoneInfo("Europe/Rome")
except Exception:  # pragma: no cover - tzdata missing
    CET = timezone(timedelta(hours=1))


def now_ms():
    return int(time.time() * 1000)


def iso_utc(ms):
    return datetime.fromtimestamp(ms / 1000, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def iso_cet(ms):
    return datetime.fromtimestamp(ms / 1000, CET).strftime("%a %d %b %H:%M %Z")


# --------------------------------------------------------------------------- http


class Client:
    def __init__(self, log):
        self.log = log
        self.last = 0.0
        self.last_markets = 0.0
        self.paused_until = 0.0  # global pause after a 429
        self.n_requests = 0

    def call(self, method, path, body=None, timeout=60):
        """One attempt. Returns (http_status, raw_bytes, headers) or raises OSError."""
        if path.startswith("/api/v1/markets") and method == "GET" and "?" not in path:
            wait = MARKETS_MIN_GAP_S - (time.time() - self.last_markets)
            if wait > 0:
                time.sleep(wait)
            self.last_markets = time.time()
        wait = max(MIN_GAP_S - (time.time() - self.last), self.paused_until - time.time())
        if wait > 0:
            time.sleep(wait)
        self.last = time.time()
        self.n_requests += 1
        req = urllib.request.Request(
            BASE + path,
            data=None if body is None else json.dumps(body, separators=(",", ":")).encode(),
            method=method,
            headers={"Content-Type": "application/json", "Accept-Encoding": "gzip", "User-Agent": UA},
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout) as f:
                status, raw, hdrs = f.status, f.read(), f.headers
        except urllib.error.HTTPError as e:
            status, raw, hdrs = e.code, e.read(), e.headers
        if (hdrs.get("Content-Encoding") or "").lower() == "gzip":
            raw = gzip.decompress(raw)
        return status, raw, hdrs


def retry_after_s(hdrs, attempt):
    base = min(120.0, 2.0 * (2 ** attempt))
    ra = hdrs.get("Retry-After") if hdrs is not None else None
    if ra:
        try:
            base = max(base, float(ra))
        except ValueError:
            pass
    return base * (0.5 + random.random())  # jitter in [0.5, 1.5)


# --------------------------------------------------------------------------- selection


def market_type(m):
    da = m.get("display_attributes") or {}
    return ((da.get("market_type") or {}).get("market") or {}).get("type")


def game_row(m):
    da = m.get("display_attributes") or {}
    mt = da.get("market_type") or {}
    teams = mt.get("teams") or {}
    labels = da.get("outcome_labels") or {}
    return {
        "game_id": (da.get("game_page") or {}).get("game_id"),
        "symbol": m["symbol"],
        "event_description": da.get("event_description"),
        "long_outcome": (mt.get("market") or {}).get("outcome"),
        "long": (labels.get("long") or {}).get("abbreviated"),
        "short": (labels.get("short") or {}).get("abbreviated"),
        "away": teams.get("away_name"),
        "home": teams.get("home_name"),
        "start_ms": int(da.get("expected_event_start_time_ms") or 0),
        "expected_resolution_ms": int(da.get("expected_resolution_time_ms") or 0),
        "mark_price_at_selection": m.get("mark_price"),
        "open_interest_at_selection": m.get("open_interest"),
    }


def overlaps(a, b):
    return a["start_ms"] < b["expected_resolution_ms"] and b["start_ms"] < a["expected_resolution_ms"]


def book_score(book):
    """USD notional resting within 5 cents of mid; 0 for an empty or one-sided book."""
    asks, bids = book.get("asks") or [], book.get("bids") or []
    if not asks or not bids:
        return 0.0
    mid = (float(asks[0][0]) + float(bids[0][0])) / 2
    s = 0.0
    for p, q in asks + bids:
        if abs(float(p) - mid) <= 0.05:
            s += float(p) * float(q)
    return s


def select_games(markets, t_ms, max_games, books=None):
    """Return (rows, note). One row per game; rows sorted by start time."""
    cands = []
    for m in markets:
        if not m.get("symbol", "").startswith("NBA_") or market_type(m) != "moneyline":
            continue
        if m.get("resolution"):
            continue
        r = game_row(m)
        if r["start_ms"] and r["game_id"]:
            cands.append((r, m))
    note = "games starting within 24 h"
    win = [c for c in cands if t_ms - 15 * 60_000 <= c[0]["start_ms"] <= t_ms + 24 * 3600_000]
    if not win:
        fut = sorted((c for c in cands if c[0]["start_ms"] > t_ms), key=lambda c: c[0]["start_ms"])
        if not fut:
            return [], "no upcoming NBA moneyline markets in the catalogue"
        first = fut[0][0]["start_ms"]
        win = [c for c in fut if c[0]["start_ms"] <= first + 12 * 3600_000]
        note = "no NBA game starts within 24 h; took the next slate (%s)" % iso_cet(first)
    # one market per game (the catalogue has one moneyline symbol per game)
    by_game = {}
    for r, m in win:
        by_game.setdefault(r["game_id"], (r, m))
    rows = []
    for r, m in by_game.values():
        st = m.get("stats") or {}
        traded = float(((st.get("all_time") or {}).get("taker_buy_volume_notional") or 0)) + float(
            ((st.get("all_time") or {}).get("taker_sell_volume_notional") or 0))
        r["score"] = round(book_score((books or {}).get(r["symbol"], {})) + 100 * traded
                           + 100 * float(m.get("open_interest") or 0), 2)
        rows.append(r)
    # round-robin across distinct tip-off times (staggered overlaps make a better
    # replay), best score first inside each slot and across slots
    slots = {}
    for r in sorted(rows, key=lambda r: -r["score"]):
        slots.setdefault(r["start_ms"], []).append(r)
    order = sorted(slots.values(), key=lambda l: -l[0]["score"])
    picked = []
    while len(picked) < max_games and any(order):
        for lst in order:
            if lst and len(picked) < max_games:
                picked.append(lst.pop(0))
    # guarantee >= 2 overlapping games when possible
    def has_overlap(p):
        return any(overlaps(a, b) for i, a in enumerate(p) for b in p[i + 1:])
    if len(picked) >= 2 and not has_overlap(picked):
        rest = [r for l in order for r in l]
        for r in sorted(rest, key=lambda r: -r["score"]):
            if any(overlaps(r, p) for p in picked):
                picked[-1] = r
                break
    picked.sort(key=lambda r: (r["start_ms"], r["symbol"]))
    return picked, note


# --------------------------------------------------------------------------- recorder


class Recorder:
    def __init__(self, a):
        self.a = a
        self.out = a.out
        self.raw = os.path.join(a.out, "raw")
        os.makedirs(self.raw, exist_ok=True)
        self.logf = open(os.path.join(self.raw, "recorder.log"), "a", buffering=1)
        self.client = Client(self.log)
        self.stop = False
        self.stop_reason = None
        self.resolved = {}  # symbol -> resolution dict
        self.dropped = set()

    def log(self, msg):
        self.logf.write("%s %s\n" % (datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")[:-4], msg))

    def append(self, kind, method, path, status, raw, extra=None):
        """Append the body verbatim; returns the parsed doc or None if not JSON."""
        try:
            doc = json.loads(raw)
        except ValueError:
            self.log("NONJSON %s %s http=%s bytes=%d head=%r" % (method, path, status, len(raw), raw[:120]))
            return None
        body = raw.decode("utf-8").strip()
        if "\n" in body:  # keep one line per response; same keys/order/strings
            body = json.dumps(doc, ensure_ascii=False, separators=(",", ":"))
        head = {"recv_ms": now_ms(), "method": method, "path": path, "http": status}
        if extra:
            head.update(extra)
        line = json.dumps(head, separators=(",", ":"))[:-1] + ',"body":' + body + "}\n"
        with open(os.path.join(self.raw, kind + ".jsonl"), "a", encoding="utf-8") as f:
            f.write(line)
        return doc

    def request(self, kind, method, path, body=None, record=True, extra=None, tries=1):
        """Returns (status, doc|None, raw). Never raises on transport errors."""
        for attempt in range(tries):
            t0 = time.time()
            try:
                status, raw, hdrs = self.client.call(method, path, body)
            except Exception as e:  # URLError, timeouts, resets, decode errors
                self.log("ERR %s %s %s: %s" % (kind, method, path[:80], type(e).__name__, e))
                if attempt < tries - 1:
                    time.sleep(retry_after_s(None, attempt))
                    continue
                return None, None, b""
            ms = int((time.time() - t0) * 1000)
            doc = self.append(kind, method, path, status, raw, extra) if record else None
            if not record:
                try:
                    doc = json.loads(raw)
                except ValueError:
                    doc = None
            seq = (doc or {}).get("seq") if isinstance(doc, dict) else None
            self.log("REQ %s %s http=%d bytes=%d ms=%d seq=%s" % (kind, method, status, len(raw), ms, seq))
            if status == 429 or status >= 500:
                pause = retry_after_s(hdrs, attempt)
                if status == 429:
                    self.client.paused_until = time.time() + pause
                self.log("BACKOFF %s http=%d %.1fs" % (kind, status, pause))
                if attempt < tries - 1:
                    time.sleep(pause)
                    continue
                return status, doc, raw
            return status, doc, raw
        return None, None, b""

    # ------------------------------------------------------------------ setup

    def setup(self):
        a = self.a
        t = now_ms()
        self.log("START pid=%d args=%s" % (os.getpid(), vars(a)))
        status, raw = None, b""
        for attempt in range(8):
            try:
                status, raw, hdrs = self.client.call("GET", "/api/v1/markets")
                self.log("REQ markets GET http=%s bytes=%d" % (status, len(raw)))
                if status == 200:
                    break
            except Exception as e:
                self.log("ERR markets: %s" % e)
            time.sleep(retry_after_s(None, attempt))
        if status != 200:
            sys.exit("could not fetch /api/v1/markets")
        with gzip.open(os.path.join(self.raw, "markets_full.json.gz"), "wb") as f:
            f.write(raw)
        cat = json.loads(raw)
        markets = cat["data"]
        # one /books call over every candidate, to rank by resting depth
        prelim, _ = select_games(markets, t, 50)
        books = {}
        syms = [r["symbol"] for r in prelim]
        for i in range(0, len(syms), 50):
            st, doc, _ = self.request("books", "GET", "/api/v1/books?" + urllib.parse.urlencode(
                {"symbols": ",".join(syms[i:i + 50])}), record=False, tries=4)
            if st == 200 and doc:
                books.update(doc["data"]["books"])
        games, note = select_games(markets, t, a.max_games, books)
        if not games:
            sys.exit("no NBA moneyline markets to record: " + note)
        self.games = games
        self.symbols = [g["symbol"] for g in games]
        for g in games:
            g["start_cet"] = iso_cet(g["start_ms"])
            g["expected_resolution_cet"] = iso_cet(g["expected_resolution_ms"])
            g["overlaps_with"] = [h["game_id"] for h in games if h is not g and overlaps(g, h)]
        pairs = [[g["game_id"], h["game_id"]] for i, g in enumerate(games) for h in games[i + 1:] if overlaps(g, h)]
        sel = dict(cat)
        sel["data"] = [m for m in markets if m["symbol"] in set(self.symbols)]
        with open(os.path.join(self.out, "markets_selected.json"), "w", encoding="utf-8") as f:
            json.dump(sel, f, indent=1, ensure_ascii=False)
            f.write("\n")
        self.manifest = {
            "tape": os.path.basename(os.path.normpath(self.out)),
            "base_url": BASE,
            "selection_time_ms": t,
            "selection_time_utc": iso_utc(t),
            "selection_note": note,
            "catalogue": {"seq": cat.get("seq"), "state_time_ms": cat.get("state_time_ms"),
                          "markets": len(markets), "nba_moneyline_candidates": len(prelim)},
            "symbols": self.symbols,
            "game_ids": [g["game_id"] for g in games],
            "games": games,
            "overlapping_pairs": pairs,
            "recorder": {"args": {k: v for k, v in vars(a).items() if k != "cmd"},
                         "pid": os.getpid(), "started_utc": iso_utc(t)},
            "files": {
                "markets_selected.json": "GET /api/v1/markets at selection, data filtered to `symbols` (envelope verbatim)",
                "raw/books.jsonl": "GET /api/v1/books?symbols=<symbols>",
                "raw/trades.jsonl": "GET /api/v1/trades?symbols=<symbols>&limit=50",
                "raw/search.jsonl": "POST /api/v1/markets/search {\"symbols\": <symbols>}",
                "raw/markets.jsonl": "hourly GET /api/v1/markets, data filtered to `symbols` (marked `trimmed`)",
                "raw/markets_full.json.gz": "the full GET /api/v1/markets body at selection, gzip",
                "raw/recorder.log": "one line per request",
            },
        }
        self.write_manifest()
        self.log("SELECTED %s (%s); overlaps=%s" % (self.symbols, note, pairs))
        last_res = max(g["expected_resolution_ms"] for g in games)
        if last_res > t + a.max_hours * 3600_000:
            self.log("WARN latest expected resolution %s is after the --max-hours deadline" % iso_utc(last_res))
        self.append_markets_filtered(cat, raw_len=len(raw))

    def append_markets_filtered(self, cat, raw_len):
        sel = dict(cat)
        sel["data"] = [m for m in cat["data"] if m["symbol"] in set(self.symbols)]
        raw = json.dumps(sel, ensure_ascii=False, separators=(",", ":")).encode()
        self.append("markets", "GET", "/api/v1/markets", 200, raw,
                    extra={"trimmed": "data %d -> %d (tape symbols only); full body %d bytes" % (
                        len(cat["data"]), len(sel["data"]), raw_len)})

    def write_manifest(self):
        p = os.path.join(self.out, "manifest.json")
        with open(p + ".tmp", "w", encoding="utf-8") as f:
            json.dump(self.manifest, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.replace(p + ".tmp", p)

    # ------------------------------------------------------------------ loop

    def live_symbols(self):
        return [s for s in self.symbols if s not in self.dropped]

    def handle_symbol_not_found(self, doc):
        try:
            if doc.get("status") == "error" and doc["data"]["code"] == "SYMBOL_NOT_FOUND":
                m = re.search(r"symbol=(\S+)", doc["data"]["message"])
                if m:
                    self.dropped.add(m.group(1))
                    self.log("DROP %s (SYMBOL_NOT_FOUND)" % m.group(1))
                    return True
        except Exception:
            pass
        return False

    def do_books(self):
        syms = self.live_symbols()
        if not syms:
            return
        st, doc, _ = self.request("books", "GET", "/api/v1/books?" + urllib.parse.urlencode({"symbols": ",".join(syms)}))
        if st == 422 and doc:
            self.handle_symbol_not_found(doc)
        return st

    def do_trades(self):
        syms = self.live_symbols()
        if not syms:
            return
        st, doc, _ = self.request("trades", "GET", "/api/v1/trades?" + urllib.parse.urlencode(
            {"symbols": ",".join(syms), "limit": "50"}))
        if st == 422 and doc:
            self.handle_symbol_not_found(doc)
        return st

    def do_search(self):
        st, doc, _ = self.request("search", "POST", "/api/v1/markets/search", {"symbols": self.symbols})
        if st == 200 and doc and isinstance(doc.get("data"), list):
            for m in doc["data"]:
                r = m.get("resolution")
                if r and m["symbol"] in self.symbols and m["symbol"] not in self.resolved:
                    self.resolved[m["symbol"]] = r
                    self.log("RESOLVED %s price=%s ts=%s" % (m["symbol"], r.get("price"), r.get("market_resolution_ts_ms")))
        return st

    def do_markets(self):
        try:
            st, raw, _ = self.client.call("GET", "/api/v1/markets")
        except Exception as e:
            self.log("ERR markets: %s" % e)
            return None
        self.log("REQ markets GET http=%s bytes=%d" % (st, len(raw)))
        if st == 200:
            try:
                self.append_markets_filtered(json.loads(raw), len(raw))
            except ValueError:
                self.log("NONJSON markets")
        return st

    def run(self):
        a = self.a
        t0 = time.time()
        deadline = t0 + a.max_hours * 3600
        first_start = min(g["start_ms"] for g in self.games) / 1000
        nxt = {"books": t0, "trades": t0 + 1, "search": t0 + 2, "markets": t0 + 3600}
        fails = {k: 0 for k in nxt}
        all_resolved_at = None
        jobs = {"books": self.do_books, "trades": self.do_trades, "search": self.do_search, "markets": self.do_markets}
        while not self.stop:
            now = time.time()
            if now >= deadline:
                self.stop_reason = "max-hours"
                break
            if all_resolved_at is None and len(self.resolved) == len(self.symbols):
                all_resolved_at = now
                self.log("ALL RESOLVED; lingering %ds" % a.linger)
            if all_resolved_at is not None and now >= all_resolved_at + a.linger:
                self.stop_reason = "all-resolved"
                break
            kind = min(nxt, key=nxt.get)
            if nxt[kind] > now:
                time.sleep(min(nxt[kind] - now, 1.0))
                continue
            try:
                st = jobs[kind]()
            except Exception as e:  # belt and braces: never die mid-night
                self.log("EXC %s %s: %s" % (kind, type(e).__name__, e))
                st = None
            if st is None or st == 429 or (st and st >= 500):
                fails[kind] += 1
                nxt[kind] = time.time() + retry_after_s(None, min(fails[kind], 6))
                continue
            fails[kind] = 0
            if kind == "books":
                every = a.warmup_every if time.time() < first_start - a.warmup_hours * 3600 else a.books_every
            elif kind == "trades":
                every = a.trades_every
            elif kind == "search":
                every = a.search_every if time.time() >= first_start + a.search_from_hours * 3600 else a.search_early_every
                if all_resolved_at is not None:
                    every = max(every, a.linger)
            else:
                every = 3600
            # fixed cadence; if we fell behind, restart from now
            nxt[kind] = max(nxt[kind] + every, time.time() + 0.5 * every)
        if self.stop and not self.stop_reason:
            self.stop_reason = "signal"
        self.log("STOP reason=%s requests=%d elapsed=%.0fs" % (self.stop_reason, self.client.n_requests, time.time() - t0))
        finalize(self.out, stop_reason=self.stop_reason, log=self.log)


# --------------------------------------------------------------------------- finalize


def scan_jsonl(path):
    """Line count, first/last body.state_time_ms, bytes."""
    n, first, last = 0, None, None
    if not os.path.exists(path):
        return {"lines": 0}
    with open(path, encoding="utf-8") as f:
        for line in f:
            try:
                d = json.loads(line)
            except ValueError:
                continue
            n += 1
            stm = (d.get("body") or {}).get("state_time_ms")
            if stm is not None:
                first = stm if first is None else first
                last = stm
    return {"lines": n, "bytes": os.path.getsize(path), "first_state_time_ms": first, "last_state_time_ms": last,
            "first_utc": first and iso_utc(int(first)), "last_utc": last and iso_utc(int(last))}


def finalize(tape, stop_reason=None, log=None):
    mp = os.path.join(tape, "manifest.json")
    man = json.load(open(mp, encoding="utf-8"))
    raw = os.path.join(tape, "raw")
    files = {k: scan_jsonl(os.path.join(raw, k + ".jsonl")) for k in KINDS}
    # resolutions: last search line that carries each symbol's resolution
    res = {}
    sp = os.path.join(raw, "search.jsonl")
    if os.path.exists(sp):
        for line in open(sp, encoding="utf-8"):
            try:
                d = json.loads(line)
            except ValueError:
                continue
            for m in (d.get("body") or {}).get("data") or []:
                if isinstance(m, dict) and m.get("resolution") and m.get("symbol") not in res:
                    res[m["symbol"]] = dict(m["resolution"], first_seen_recv_ms=d.get("recv_ms"))
    # book coverage per symbol: first/last state_time_ms with a non-empty book
    cov = {}
    bp = os.path.join(raw, "books.jsonl")
    if os.path.exists(bp):
        for line in open(bp, encoding="utf-8"):
            try:
                d = json.loads(line)
            except ValueError:
                continue
            body = d.get("body") or {}
            stm = body.get("state_time_ms")
            for s, b in (((body.get("data") or {}).get("books")) or {}).items():
                c = cov.setdefault(s, {"snapshots": 0, "nonempty": 0, "first_state_time_ms": stm})
                c["snapshots"] += 1
                c["last_state_time_ms"] = stm
                if b.get("asks") or b.get("bids"):
                    c["nonempty"] += 1
                    c.setdefault("first_nonempty_state_time_ms", stm)
                    c["last_nonempty_state_time_ms"] = stm
    for g in man.get("games", []):
        r = res.get(g["symbol"])
        g["resolution"] = r
        c = cov.get(g["symbol"], {})
        g["books"] = c
        if c and c.get("first_state_time_ms"):
            g["books_from_before_tipoff"] = int(c["first_state_time_ms"]) < g["start_ms"]
            if r:
                g["books_after_resolution"] = int(c["last_state_time_ms"]) > int(r["market_resolution_ts_ms"])
    man["resolutions"] = {s: (res[s]["price"] if s in res else None) for s in man.get("symbols", [])}
    man["raw_files"] = files
    fsts = [v["first_state_time_ms"] for v in files.values() if v.get("first_state_time_ms")]
    lsts = [v["last_state_time_ms"] for v in files.values() if v.get("last_state_time_ms")]
    man["first_state_time_ms"] = min(fsts, key=int) if fsts else None
    man["last_state_time_ms"] = max(lsts, key=int) if lsts else None
    man.setdefault("recorder", {})
    man["recorder"]["finalized_utc"] = iso_utc(now_ms())
    if stop_reason:
        man["recorder"]["stop_reason"] = stop_reason
    with open(mp + ".tmp", "w", encoding="utf-8") as f:
        json.dump(man, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.replace(mp + ".tmp", mp)
    if log:
        log("FINALIZED manifest: %s" % {k: v.get("lines") for k, v in files.items()})
    return man


# --------------------------------------------------------------------------- trim


def read_lines(path):
    if not os.path.exists(path):
        return []
    out = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.strip():
                out.append(line if line.endswith("\n") else line + "\n")
    return out


def evenly(idx_count, k):
    if k >= idx_count:
        return list(range(idx_count))
    if k <= 1:
        return [0]
    return sorted({round(i * (idx_count - 1) / (k - 1)) for i in range(k)})


def trim(tape, out, max_mb):
    budget = int(max_mb * 1024 * 1024)
    raw = os.path.join(tape, "raw")
    os.makedirs(out, exist_ok=True)
    books = read_lines(os.path.join(raw, "books.jsonl"))
    trades = read_lines(os.path.join(raw, "trades.jsonl"))
    search = read_lines(os.path.join(raw, "search.jsonl"))
    markets = read_lines(os.path.join(raw, "markets.jsonl"))

    # search: the first line (pre-game metadata) and every line where some
    # symbol's resolution first appears (or changes)
    keep_search, seen = [], {}
    for i, line in enumerate(search):
        d = json.loads(line)
        new = False
        for m in (d.get("body") or {}).get("data") or []:
            if isinstance(m, dict) and m.get("resolution") is not None:
                key = json.dumps(m["resolution"], sort_keys=True)
                if seen.get(m.get("symbol")) != key:
                    seen[m.get("symbol")] = key
                    new = True
        if i == 0 or new:
            keep_search.append(line)
    if search and search[-1] not in keep_search:
        keep_search.append(search[-1])
    # trades: lines whose item set differs from the previous kept one
    keep_trades, prev = [], None
    for line in trades:
        d = json.loads(line)
        ids = tuple(it.get("trade_id") for it in (((d.get("body") or {}).get("data") or {}).get("items") or []))
        if ids != prev:
            keep_trades.append(line)
            prev = ids
    keep_markets = markets[:1]
    fixed = sum(len(x.encode()) for x in keep_search + keep_markets)
    trade_cap = max(0, (budget - fixed) // 5)
    if sum(len(x.encode()) for x in keep_trades) > trade_cap and keep_trades:
        avg = sum(len(x.encode()) for x in keep_trades) / len(keep_trades)
        keep_trades = [keep_trades[i] for i in evenly(len(keep_trades), max(1, int(trade_cap / avg)))]
    used = fixed + sum(len(x.encode()) for x in keep_trades)
    # books: the snapshots bracketing each symbol's book going empty (resolution),
    # then evenly thinned to fill the remaining budget
    must = {0, len(books) - 1} if books else set()
    last_nonempty = {}
    for i, line in enumerate(books):
        d = json.loads(line)
        for s, b in ((((d.get("body") or {}).get("data") or {}).get("books")) or {}).items():
            if b.get("asks") or b.get("bids"):
                last_nonempty[s] = i
    for s, i in last_nonempty.items():
        must.add(i)
        if i + 1 < len(books):
            must.add(i + 1)
    remaining = budget - used - 4096  # headroom for manifest
    if books:
        avg = sum(len(x.encode()) for x in books) / len(books)
        k = max(len(must), int(remaining / avg))
        pick = set(evenly(len(books), max(2, k - len(must)))) | must
        while k > 2 and len(pick) > len(must) and sum(len(books[i].encode()) for i in pick) > remaining:
            k = int(k * 0.95)
            pick = set(evenly(len(books), max(2, k - len(must)))) | must
        keep_books = [books[i] for i in sorted(pick)]
    else:
        keep_books = []
    for name, lines in (("books", keep_books), ("trades", keep_trades), ("search", keep_search), ("markets", keep_markets)):
        with open(os.path.join(out, name + ".jsonl"), "w", encoding="utf-8") as f:
            f.writelines(lines)
    man = {}
    mp = os.path.join(tape, "manifest.json")
    if os.path.exists(mp):
        man = json.load(open(mp, encoding="utf-8"))
    man["sample"] = {
        "made_utc": iso_utc(now_ms()),
        "max_mb": max_mb,
        "source_lines": {"books": len(books), "trades": len(trades), "search": len(search), "markets": len(markets)},
        "kept_lines": {"books": len(keep_books), "trades": len(keep_trades), "search": len(keep_search),
                       "markets": len(keep_markets)},
        "rules": {
            "books": "evenly thinned over the whole tape, always keeping the first and last line and, per symbol, "
                     "the last snapshot with a non-empty book and the one after it",
            "trades": "lines whose list of trade_ids differs from the previous kept line (thinned evenly if over 20% of budget)",
            "search": "first line, every line where a symbol's resolution first appears or changes, last line",
            "markets": "first hourly line (already filtered to the tape symbols)",
        },
        "lines_verbatim": True,
    }
    with open(os.path.join(out, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(man, f, indent=2, ensure_ascii=False)
        f.write("\n")
    total = sum(os.path.getsize(os.path.join(out, n)) for n in os.listdir(out))
    print(json.dumps({"out": out, "bytes": total, **man["sample"]["kept_lines"]}))
    return total


# --------------------------------------------------------------------------- main


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    cmd = argv.pop(0) if argv and argv[0] in ("record", "trim", "finalize") else "record"
    ap = argparse.ArgumentParser(prog="record_tape.py " + cmd)
    if cmd == "record":
        ap.add_argument("--out", required=True)
        ap.add_argument("--max-games", type=int, default=5)
        ap.add_argument("--books-every", type=float, default=5)
        ap.add_argument("--trades-every", type=float, default=30)
        ap.add_argument("--search-every", type=float, default=60)
        ap.add_argument("--search-early-every", type=float, default=600,
                        help="search cadence before first tip-off + --search-from-hours")
        ap.add_argument("--search-from-hours", type=float, default=2.0)
        ap.add_argument("--warmup-hours", type=float, default=2.0,
                        help="books run at --warmup-every until this long before the first tip-off")
        ap.add_argument("--warmup-every", type=float, default=30)
        ap.add_argument("--linger", type=float, default=120, help="seconds to keep recording after all resolve")
        ap.add_argument("--max-hours", type=float, default=14)
        a = ap.parse_args(argv)
        rec = Recorder(a)

        def on_sig(signum, _frame):
            rec.stop = True
            rec.stop_reason = "signal %d" % signum
        signal.signal(signal.SIGTERM, on_sig)
        signal.signal(signal.SIGINT, on_sig)
        signal.signal(signal.SIGHUP, signal.SIG_IGN)
        with open(os.path.join(rec.raw, "recorder.pid"), "w") as f:
            f.write("%d\n" % os.getpid())
        rec.setup()
        rec.run()
    elif cmd == "trim":
        ap.add_argument("--tape", required=True)
        ap.add_argument("--out", required=True)
        ap.add_argument("--max-mb", type=float, default=1.0)
        a = ap.parse_args(argv)
        trim(a.tape, a.out, a.max_mb)
    else:
        ap.add_argument("--tape", required=True)
        a = ap.parse_args(argv)
        man = finalize(a.tape)
        print(json.dumps({"resolutions": man["resolutions"], "raw_files": man["raw_files"]}, indent=1))


if __name__ == "__main__":
    main()
