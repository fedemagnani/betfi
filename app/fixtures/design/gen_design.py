"""Write app/fixtures/design/: the BetFi C2 mockup's content as desk answers (v2, 7.0a).

Usage, from app/:  python3 -I fixtures/design/gen_design.py fixtures/design

FICTIONAL DATA, for the side-by-side visual review only. EuroLeague and Champions League games,
@lorenzo_hoops, the copiers, the balance and every amount are the mockup's, not Pascal's or the
desk's; wallets, ticket ids and signatures are random base58 strings that own nothing on any
cluster. Loaded only by a build with SCHEDINA_DESIGN_FIXTURES set (src/config.rs), never in demo
or live mode (D-314).

Data: prompts/l7-contracts.md L7-2 (the mockup "BetFi C2", read after its guidelines).
Shapes: the L7-1 API shapes (l7-contracts L7-1) on top of L6-2's, including the new optional
fields: games[].sport / league, winner picks' outcome ("1", "X", "2"; 1 = home), the winner
market's market_title, /fx's eur_per_sol_ppm, /profile's verified / links / followers /
following, open tips' pool_u / copies / teams, the copiers answer, /stream feed frames with who /
amount_u / team / odds_u, /sentiment's traders / recent, Ticket.tx.slot / confirm_ms. The app's
types ignore the fields they do not know yet (serde), so the current screens still render.

Known differences from the mockup, where its screens disagree with each other or with the
guidelines (each L7-1 step lists the ones it sees):
- One home team per game, outcome 1 = home (L7-1): Panathinaikos (home, 1) 2.35, Fenerbahçe
  (away, 2) 1.62, as the sportsbook draws them; the event screen's "Fenerbahçe [1]" and the copy
  screen's "Panathinaikos 1.62" are not reproduced.
- Consistent totals (7.2a-p, l7-contracts L7-5 amendment): every Total is the desk's offered
  price, C2 on the legs' prices with the per-leg margin m1 (3% for a leg whose game overlaps no
  other leg's, 5% otherwise; overlap = starts less than 2.5 h apart, crates/pricing), so it is
  a little below the product of the legs; every payout is stake x Total, to the cent shown. To
  keep the mockup's Total 4.55 and payout EUR 113.75 on the drawer and on the copy (both three
  legs, two overlapping: 0.95 x 0.95 x 0.97), the card's prices move from the mockup's: Panathinaikos
  2.35 -> 1.96, Real Madrid 1.52 -> 1.56, Celtics 1.71 -> 1.70, Arsenal 1.95 -> 1.96
  (1.96 x 1.56 x 1.70 x 0.875425 = 4.5504). The mockup's own legs multiply to 6.11 and 5.07.
- A card price p_u is floor(1e6 / odds) and its odds_u the desk's odds_at(p_u) = floor(1e12 /
  p_u), so the app's truncated and rounded readings both give the two-decimal value.
- The slip (bet drawer) holds the three picks at the card's prices (1.96, 1.56, 1.70); the
  drawer draws Celtics at 1.28. Its seeded "seen" total is 4.62 (the drawer's quote before
  Panathinaikos moved from 1.99), so the drawer opens with the price-moved banner (D-492);
  Accept shows the mockup's Total 4.55 and payout EUR 113.75.
- Master Treble's legs are Real Madrid, Boston Celtics and Arsenal, all before tip-off: the app
  (and the program) refuse a copy once a leg has started, so the copy screen's live
  Panathinaikos leg is not reproduced; its team chips read RM BOS ARS (the profile draws
  RM PAO OLY). The tips' and the settled bets' leg prices are solved for their shown totals
  (solve_last()); a tip's legs were priced when it was posted, so they may differ from the
  card's by a micro-unit.
- The draw pick of the soccer game has "team": null (DRAW_TEAM): the app's `Pick` enum has no
  "draw" yet, and a value it does not know would fail the whole card. Set DRAW_TEAM = "draw"
  once app/src/desk/types.rs accepts it (follow-up for 7.0c / 7.1a).
- Handles follow the desk's rule ^[a-z0-9_]{3,20}$: "degencall_sol" for the mockup's
  "degencall.sol".
- SOL at EUR 135.80 (L7-2): the balance EUR 1,240.50 is 9.13 SOL (the mockup prints 9.12), the
  pool EUR 28,450 is 209.5 SOL (the mockup prints 208.8).

Money: the app shows EUR = USDC x eur_per_usd_ppm, truncated to the cent (L6-2). FX_PPM keeps
L6's exact pairs (EUR 1,240.50 = 1,346.81 USDC, EUR 25.00 = 27.143 USDC, EUR 61.20 = 66.45
USDC). Every amount is checked below (check()).

Times: the mockup's clock is Italy's (CET, UTC+1 on 29 Oct 2026); the screenshot script renders
in Europe/Rome. "Now" (as_of_ms) is 20:20 CET; Panathinaikos-Fenerbahçe started at 19:30.
"""
import json
import os
import random
import sys
from datetime import datetime, timedelta, timezone

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(os.path.abspath(__file__))
NOTE = "BetFi C2 mockup content: fictional, for visual review only (7.0a design fixtures v2)"

FX_PPM = 921_068
SOL_PPM = 135_800_000          # EUR 135.80 per SOL (L7-2)
DRAW_TEAM = None               # becomes "draw" when the app's Pick enum accepts it
CET = timezone(timedelta(hours=1))
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
rng = random.Random(7_0_0)  # fixed seed: the same ids on every run


def b58_id(prefix="", suffix=""):
    """A random 32-byte value in base58 (an address-shaped id that owns nothing), optionally
    with a given start and end (the mockup's short addresses, "7xK9…94Lp")."""
    n = rng.getrandbits(256) | (1 << 255)
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = B58[r] + out
    return prefix + out[len(prefix):len(out) - len(suffix)] + suffix


def sig():
    return b58_id() + b58_id()[:44]


def ms(day, hh, mm):
    return int(datetime(2026, 10, day, hh, mm, tzinfo=CET).timestamp() * 1000)


def eur_u(usdc_u):
    return usdc_u * FX_PPM // 1_000_000


def eur_text(usdc_u):
    e = eur_u(usdc_u)
    return f"€{e // 1_000_000:,}.{(e % 1_000_000) // 10_000:02}"


def usdc_text(usdc_u):
    return f"{usdc_u // 1_000_000:,}.{(usdc_u % 1_000_000) // 10_000:02}"


def usdc_for_eur(eur):
    """The smallest USDC amount whose euro value is `eur` exactly (ceiling division)."""
    return -(-round(eur * 1_000_000) * 1_000_000 // FX_PPM)


def odds_u(odds):
    return round(odds * 1_000_000)


def cents(odds):
    c = round(odds * 100)
    assert abs(c - odds * 100) < 1e-6, odds
    return c


def price_u(odds):
    """The price behind a two-decimal value: floor(1e6 / odds), so odds_at() reads it back."""
    return 100_000_000 // cents(odds)


def odds_at(p_u):
    """The desk's card odds for a price (crates/desk api/types.rs odds_at)."""
    return 1_000_000_000_000 // p_u


def card_pick(odds):
    p = price_u(odds)
    return {"price_u": p, "odds_u": odds_at(p)}


# ---------------------------------------------------------------- C2 (crates/pricing payout.rs)

U = 1_000_000
GAME_LENGTH_MS = 5 * 3_600_000 // 2      # the overlap window (NBA 2.5 h, D-460)
SEQ_PPM, OVERLAP_PPM = 30_000, 50_000     # m1 tiers (spec 2 step 3)


def m1s(starts):
    """Per-leg m1: the overlap tier when another leg starts less than 2.5 h away."""
    return [OVERLAP_PPM if any(j != i and abs(a - b) < GAME_LENGTH_MS for j, b in enumerate(starts))
            else SEQ_PPM for i, a in enumerate(starts)]


def c2(stake_u, legs):
    """C2 with r = 1: legs = [(m1_ppm, f_u)]; returns (payout_u, odds_u)."""
    acc = stake_u
    for m, f in legs:
        acc = acc * (U - m) // f
    return acc, acc * U // stake_u


def m_n_ppm(ms):
    keep = U
    for m in ms:
        keep = keep * (U - m) // U
    return U - keep


def total_text(odds_u_):
    return f"{odds_u_ // U}.{(odds_u_ % U) // 10_000:02}"


def priced(stake_u, starts, prices, total, exact=True):
    """C2 on `prices`; asserts it shows `total` and (`exact`) pays stake x total to the cent."""
    payout, odds = c2(stake_u, list(zip(m1s(starts), prices)))
    assert total_text(odds) == f"{total:.2f}", (total, odds, prices)
    if exact:
        assert eur_text(payout) == eur_text(stake_u * odds_u(total) // U), (total, payout, stake_u)
    return payout, odds


def solve_last(stake_u, starts, prices, total, near):
    """The last leg's price for `total` (shown, and stake x total to the cent), the two-decimal
    value nearest `near` that allows one: its own price floor(1e6 / d) when that works, else a
    price whose odds lie in [d, d + 0.005), so the app's truncated and rounded readings of the
    leg both say d."""
    ms = m1s(starts)

    def ok(p):
        payout, odds = c2(stake_u, list(zip(ms, prices + [p])))
        return (total_text(odds) == f"{total:.2f}"
                and eur_text(payout) == eur_text(stake_u * odds_u(total) // U))
    base = cents(near)
    for d in sorted(range(base - 40, base + 41), key=lambda c: abs(c - base)):
        top = 100_000_000 // d                    # odds exactly d (or a hair above)
        low = -(-200_000_000 // (2 * d + 1))      # odds just under d + 0.005
        for p in range(top, low - 1, -1):
            if ok(p):
                return p
    return None


def must(price):
    assert price is not None, "no price"
    return price


def solve_two(stake_u, starts, prices, total, near_a, near_b):
    """Like solve_last() for the last two legs: the next-to-last leg at the two-decimal value
    nearest `near_a` that lets the last leg (near `near_b`) give `total`; returns both prices."""
    base = cents(near_a)
    for d in sorted(range(base - 10, base + 11), key=lambda c: abs(c - base)):
        a = 100_000_000 // d
        b = solve_last(stake_u, starts, prices + [a], total, near_b)
        if b is not None:
            return a, b
    raise AssertionError(("no prices for", total, prices))


def check(usdc_u, eur, usdc=None):
    assert eur_text(usdc_u) == eur, (usdc_u, eur_text(usdc_u), eur)
    if usdc is not None:
        assert usdc_text(usdc_u) == usdc, (usdc_u, usdc_text(usdc_u), usdc)


AS_OF = ms(29, 20, 20)
USER = b58_id()
LORENZO = b58_id()
GIULIA = b58_id()
MARCO = b58_id()

# ---------------------------------------------------------------- the card

def team(name, abbr, short=None):
    return {"name": name, "abbr": abbr, "short": short or name}


def winner(sport, home, away, h_odds, a_odds, sym, title, d_odds=None, code=None):
    """The winner market, picks in outcome order: home (1), draw (X, soccer), away (2)."""
    if sport == "soccer":
        picks = [
            {"label": home["short"], "team": "home", "outcome": "1", "symbol": f"{code}.{home['abbr']}",
             "side": "long", **card_pick(h_odds)},
            {"label": "Draw", "team": DRAW_TEAM, "outcome": "X", "symbol": f"{code}.DRAW",
             "side": "long", **card_pick(d_odds)},
            {"label": away["short"], "team": "away", "outcome": "2", "symbol": f"{code}.{away['abbr']}",
             "side": "long", **card_pick(a_odds)},
        ]
    else:
        # D-021: one symbol per game, the away team's long; a home pick is its short.
        picks = [
            {"label": home["short"], "team": "home", "outcome": "1", "symbol": sym, "side": "short",
             **card_pick(h_odds)},
            {"label": away["short"], "team": "away", "outcome": "2", "symbol": sym, "side": "long",
             **card_pick(a_odds)},
        ]
    return {"kind": "winner", "line": None, "market_title": title, "picks": picks}


def spread(away, home, line, a_odds, h_odds, sym):
    # `line` is the away team's line; the home team carries the opposite.
    opp = line[1:] if line.startswith("-") else "-" + line.lstrip("+")
    opp = opp if opp.startswith("-") else "+" + opp
    return {"kind": "spread", "line": line, "picks": [
        {"label": f"{away['short']} {line}", "team": "away", "symbol": sym, "side": "long",
         **card_pick(a_odds)},
        {"label": f"{home['short']} {opp}", "team": "home", "symbol": sym, "side": "short",
         **card_pick(h_odds)}]}


def total(line, o_odds, u_odds, sym):
    return {"kind": "total", "line": line, "picks": [
        {"label": f"Over {line}", "team": None, "symbol": sym, "side": "long",
         **card_pick(o_odds)},
        {"label": f"Under {line}", "team": None, "symbol": sym, "side": "short",
         **card_pick(u_odds)}]}


GAMES = []
PREFIX = {"EuroLeague": "EL", "NBA": "NBA", "Champions League": "UCL"}


def game(gid, sport, league, home, away, start, h_odds, a_odds, count, title, extra=(),
         in_play=False, d_odds=None, score=None):
    p = PREFIX[league]
    day = datetime.fromtimestamp(start / 1000, CET).strftime("%d").lstrip("0")
    code = f"{p}_26OCT{day}_{away['abbr']}{home['abbr']}"
    if sport == "soccer":
        code = f"{p}_26OCT{day}_{home['abbr']}{away['abbr']}_ML"
    sym = f"{code}_ML.{away['abbr']}" if sport != "soccer" else f"{code}.{home['abbr']}"
    markets = [winner(sport, home, away, h_odds, a_odds, sym, title, d_odds, code)]
    for build in extra:
        markets.append(build(away, home, code))
    home_pick = markets[0]["picks"][0]
    away_pick = markets[0]["picks"][-1]
    g = {
        "game_id": gid,
        "sport": sport,
        "league": league,
        "symbol": sym,
        "start_ms": start,
        "expected_resolution_ms": start + 2 * 3600 * 1000,
        "away": {"name": away["name"], "abbr": away["abbr"]},
        "home": {"name": home["name"], "abbr": home["abbr"]},
        "long_team": "away" if sport != "soccer" else "home",
        "picks": [
            {"pick": "home", "side": home_pick["side"], "price_u": home_pick["price_u"], "odds_u": home_pick["odds_u"]},
            {"pick": "away", "side": away_pick["side"], "price_u": away_pick["price_u"], "odds_u": away_pick["odds_u"]},
        ],
        "markets_count": count,
        "in_play": in_play,
        "started_ms": start if in_play else None,
        "markets": markets,
    }
    if score:
        g["score"] = score
    GAMES.append(g)
    return g


PAO = team("Panathinaikos", "PAO")
FEN = team("Fenerbahçe", "FEN")
RMA = team("Real Madrid", "RMA")
OLY = team("Olympiacos", "OLY")
BOS = team("Boston Celtics", "BOS")
NYK = team("NY Knicks", "NYK")
ARS = team("Arsenal", "ARS")
BAY = team("Bayern Munich", "BAY")

# The sportsbook as drawn: the live EuroLeague hero, then "Upcoming matches · Today". Game ids
# stay those the visual shots know.
# Prices: the mockup's, except where the drawer's and the copy's Total 4.55 need others (see
# the module docs): Panathinaikos 2.35 -> 1.96, Real Madrid 1.52 -> 1.56, Celtics 1.71 -> 1.70,
# Arsenal 1.95 -> 1.96.
g_pao = game("el-fen-pao-2026-10-29", "basketball", "EuroLeague", PAO, FEN, ms(29, 19, 30),
             1.96, 1.62, 1, "Head to Head", in_play=True,
             score={"home": 61, "away": 66, "period": "Q3", "elapsed": "4:12"})
g_rma = game("el-rma-oly-2026-10-29", "basketball", "EuroLeague", RMA, OLY, ms(29, 20, 45),
             1.56, 2.55, 25, "Head to Head", extra=(
                 lambda a, h, p: spread(a, h, "+3.5", 1.92, 1.88, p + "_SP.OLY_3"),
                 lambda a, h, p: total("161.5", 1.90, 1.90, p + "_TP_161"),
             ))
g_bos = game("nba-bos-nyk-2026-10-29", "basketball", "NBA", BOS, NYK, ms(30, 1, 0),
             1.70, 2.18, 32, "Moneyline", extra=(
                 lambda a, h, p: spread(a, h, "+3.5", 1.90, 1.90, p + "_SP.NYK_3"),
                 lambda a, h, p: total("221.5", 1.91, 1.89, p + "_TP_221"),
             ))
g_ars = game("ucl-ars-bay-2026-10-29", "soccer", "Champions League", ARS, BAY, ms(29, 21, 0),
             1.96, 3.80, 3, "Full Time Result (1X2)", d_odds=3.60)

CARD = {"_fixture": NOTE, "sport": "all", "date": "2026-10-29", "as_of_ms": AS_OF, "games": GAMES}


def pick_of(g, idx):
    return g["markets"][0]["picks"][idx]

# ---------------------------------------------------------------- the slip (bet drawer)

STAKE_25 = 27_143_000          # EUR 25.00
check(STAKE_25, "€25.00")
SLIP_PICKS = [(g_pao, 0), (g_rma, 0), (g_bos, 0)]   # each game's outcome 1, at the card's price
TOTAL = 4.55


def quote_leg(g, pick, m1):
    return {"symbol": pick["symbol"], "game_id": g["game_id"], "side": pick["side"],
            "pick": pick["team"] or "home", "start_ms": g["start_ms"],
            "price_u": pick["price_u"], "p_max_u": pick["price_u"] + 10_000,
            "m1_ppm": m1, "overlaps": m1 == OVERLAP_PPM, "in_play": g["in_play"]}


def quote(picks, stake_u, total, copy_fee_u=0):
    """The desk's quote for `picks` [(game, pick)]: C2 at the picks' prices (the offered total,
    which must show as `total`), the fair payout at the same prices without margin, and
    fee_u = fair - offered (D-032)."""
    starts = [g["start_ms"] for g, _ in picks]
    prices = [p["price_u"] for _, p in picks]
    payout, odds = priced(stake_u, starts, prices, total)
    fair, fair_odds = c2(stake_u, [(0, f) for f in prices])
    legs = [quote_leg(g, p, m) for (g, p), m in zip(picks, m1s(starts))]
    return {
        "_fixture": NOTE,
        "n": len(legs), "stake_u": stake_u, "legs": legs,
        "odds_fair_u": fair_odds, "odds_offered_u": odds,
        "payout_u": payout,
        "fee_u": fair - payout, "pascal_fees_est_u": 120_000, "copy_fee_u": copy_fee_u,
        "slippage_bps": 300, "min_payout_per_unit_u": odds * (10_000 - 300) // 10_000,
        "quote_expires_at": AS_OF // 1000 + 10,
        "quote_signer": "design-fixture-not-signed",
        "quote_msg": "", "quote_sig": "",
    }


# The drawer's EUR 25.00 at Total 4.55 pays EUR 113.75: the quote's stake in euros is exact.
QUOTE_STAKE = usdc_for_eur(25)
# What the drawer showed before Panathinaikos moved from 1.99 to 1.96 (the seeded "seen" total).
SEEN_ODDS = c2(QUOTE_STAKE, list(zip(m1s([g["start_ms"] for g, _ in SLIP_PICKS]),
                                     [price_u(1.99)] + [pick_of(g, i)["price_u"] for g, i in SLIP_PICKS[1:]])))[1]
assert total_text(SEEN_ODDS) == "4.62", SEEN_ODDS
QUOTE = quote([(g, pick_of(g, i)) for g, i in SLIP_PICKS], QUOTE_STAKE, TOTAL)
assert eur_text(QUOTE["payout_u"]) == "€113.75", eur_text(QUOTE["payout_u"])
SLIP = {
    "_fixture": NOTE,
    "kind": "parlay",
    "stake_u": QUOTE_STAKE,
    "presets_eur": [10, 25, 50, 100],
    "always_accept_better": True,
    # The total the user last saw: the drawer's quote while Panathinaikos stood at 1.99 (4.62),
    # above the quote's 4.55, so the drawer opens with the price-moved banner (D-492).
    "odds_before_u": SEEN_ODDS,
    "picks": [
        {"game_id": g["game_id"], "kind": "winner", "line": None, "symbol": pick_of(g, i)["symbol"],
         "side": pick_of(g, i)["side"], "label": pick_of(g, i)["label"], "outcome": pick_of(g, i)["outcome"],
         "odds_u": pick_of(g, i)["odds_u"], "odds_before_u": None}
        for g, i in SLIP_PICKS
    ],
}

# ---------------------------------------------------------------- tickets (celebrations, my bets)

BALANCE = 1_346_810_000
check(BALANCE, "€1,240.50", "1,346.81")


def ticket_leg(idx, g, pick, status, label=None):
    m = g["markets"][0]
    return {
        "idx": idx, "symbol": pick["symbol"], "game_id": g["game_id"],
        "away": g["away"]["name"], "home": g["home"]["name"],
        "pick": pick["team"] or "home", "side": pick["side"], "start_ms": g["start_ms"],
        "p_quote_u": pick["price_u"], "p_fill_u": pick["price_u"], "hedged": True,
        "status": status, "resolution_u": 1_000_000 if status == "won" else None,
        "kind": m["kind"], "line": m["line"], "label": label or pick["label"],
    }


def tune(g, idx, p):
    """Pick `idx` of g's winner market at price `p` (a game off tonight's card); returns it."""
    pick = g["markets"][0]["picks"][idx]
    pick.update(price_u=p, odds_u=odds_at(p))
    return pick


def ticket(tid, user, legs, stake_u, total, cash_out, minted, exact=True, **extra):
    """A ticket whose payout and Total are C2 on its legs' prices (p_quote_u = p_fill_u) and
    show as `total` (`exact`: its payout is stake x total to the cent); fee_u = rS x m(n) (D-172)."""
    starts = [l["start_ms"] for l in legs]
    prices = [l["p_fill_u"] for l in legs]
    payout, odds = priced(stake_u, starts, prices, total, exact)
    _, fair_odds = c2(stake_u, [(0, f) for f in prices])
    t = {
        "ticket": tid, "user": user, "nonce": rng.getrandbits(53), "state": "open",
        "n": len(legs), "stake_u": stake_u, "payout_u": payout,
        "odds_offered_u": odds, "odds_fair_u": fair_odds,
        "fee_u": stake_u * m_n_ppm(m1s(starts)) // U, "minted_at": minted, "settled_at": None, "paid_u": None,
        "cash_out": cash_out, "legs": legs, "fills": [],
        "tx": {"open_pending": sig(), "finalize": sig(), "settle": None, "refund_pending": None,
               "cash_out": None, "resolve_leg": [],
               "slot": {"open_pending": 284_912_041 - rng.randint(1, 9_000), "cash_out": None, "settle": None},
               "confirm_ms": {"open_pending": None, "cash_out": None}},
        "tipster": None, "copied_from": None, "title": None, "posted_odds_u": odds,
    }
    t.update(extra)
    return t


# "Schedina Locked In!": the drawer's slip, placed. Slot 284,912,041, confirmed in 412 ms.
PLACED = ticket(b58_id(), USER, [ticket_leg(i, g, pick_of(g, j), "open") for i, (g, j) in enumerate(SLIP_PICKS)],
                QUOTE_STAKE, TOTAL, {"available": True, "c_u": 30_120_000, "reason": None}, AS_OF // 1000 - 60)
PLACED["tx"]["slot"]["open_pending"] = 284_912_041
PLACED["tx"]["confirm_ms"]["open_pending"] = 412
assert eur_text(PLACED["payout_u"]) == "€113.75"

# "Cash Out Secured!": Real Madrid vs Manchester City (Full Time Result), EUR 61.20 back.
MCI = team("Manchester City", "MCI")
g_mci = {"game_id": "ucl-rma-mci-2026-10-28", "start_ms": ms(28, 21, 0), "in_play": False,
         "away": {"name": "Manchester City", "abbr": "MCI"}, "home": {"name": "Real Madrid", "abbr": "RMA"},
         "markets": [winner("soccer", RMA, MCI, 2.10, 3.30, None, "Full Time Result (1X2)", 3.40,
                            "UCL_26OCT28_RMAMCI_ML")]}
CASH_OUT = 66_450_000
check(CASH_OUT, "€61.20", "66.45")
STAKE_20 = 21_714_000
check(STAKE_20, "€20.00")
# Total 2.10 on a single (3%): Real Madrid at 2.17.
tune(g_mci, 0, must(solve_last(STAKE_20, [g_mci["start_ms"]], [], 2.10, near=2.17)))
CASHED = ticket(b58_id(), USER, [ticket_leg(0, g_mci, pick_of(g_mci, 0), "open")], STAKE_20, 2.10,
                {"available": False, "c_u": None, "reason": "ticket cashed_out"}, ms(28, 20, 30) // 1000,
                state="cashed_out", paid_u=CASH_OUT, settled_at=ms(28, 22, 5) // 1000)
CASHED["tx"].update({"cash_out": sig()})
CASHED["tx"]["slot"]["cash_out"] = 892_104_821
CASHED["tx"]["confirm_ms"]["cash_out"] = 380

# Settled, yesterday: one won single, one lost double.


def past_game(gid, home, away, start, h_odds, a_odds):
    return {"game_id": gid, "start_ms": start, "in_play": False,
            "away": {"name": away["name"], "abbr": away["abbr"]}, "home": {"name": home["name"], "abbr": home["abbr"]},
            "markets": [winner("basketball", home, away, h_odds, a_odds,
                               f"EL_26OCT28_{away['abbr']}{home['abbr']}_ML.{away['abbr']}", "Head to Head")]}


g_y1 = past_game("el-mil-bar-2026-10-28", team("Barcelona", "BAR"), team("Milan", "MIL"), ms(28, 18, 45), 1.70, 2.20)
g_y2 = past_game("el-asv-mon-2026-10-28", team("Monaco", "MON"), team("ASVEL", "ASV"), ms(28, 20, 0), 1.45, 2.80)
STAKE_10 = 10_857_000
check(STAKE_10, "€10.00")
# Total 1.70 on the won single (Barcelona), 4.76 on the lost double (Barcelona, then ASVEL; the
# two games overlap: 5% each).
tune(g_y1, 0, must(solve_last(STAKE_20, [g_y1["start_ms"]], [], 1.70, near=1.75)))
tune(g_y2, 1, must(solve_last(STAKE_10, [g_y1["start_ms"], g_y2["start_ms"]], [pick_of(g_y1, 0)["price_u"]], 4.76,
                              near=3.01)))


def settled(t, paid_u, settled_at):
    t.update({"state": "settled", "paid_u": paid_u, "settled_at": settled_at,
              "cash_out": {"available": False, "c_u": None, "reason": "ticket settled"}})
    t["tx"].update({"settle": sig(), "resolve_leg": [sig() for _ in t["legs"]]})
    t["tx"]["slot"]["settle"] = 284_640_000 + rng.randint(1, 9_000)
    return t


WON = ticket(b58_id(), USER, [ticket_leg(0, g_y1, pick_of(g_y1, 0), "won")], STAKE_20, 1.70,
             None, ms(28, 17, 0) // 1000)
WON = settled(WON, WON["payout_u"], ms(28, 21, 0) // 1000)
LOST_LEGS = [ticket_leg(0, g_y1, pick_of(g_y1, 0), "won"), ticket_leg(1, g_y2, pick_of(g_y2, 1), "lost")]
LOST_LEGS[1]["resolution_u"] = 0
LOST = settled(ticket(b58_id(), USER, LOST_LEGS, STAKE_10, 4.76, None, ms(28, 17, 30) // 1000),
               0, ms(28, 22, 30) // 1000)
TICKETS = {"_fixture": NOTE, "user": USER, "tickets": [PLACED, CASHED, WON, LOST]}
CASHOUT_QUOTE = {"_fixture": NOTE, "ticket": PLACED["ticket"], "c_u": PLACED["cash_out"]["c_u"],
                 "ts": AS_OF // 1000, "quote_signer": "design-fixture-not-signed",
                 "quote_msg": "", "quote_sig": ""}

# ---------------------------------------------------------------- the tipster and the copiers

OWN = {500: usdc_for_eur(500), 350: usdc_for_eur(350), 200: usdc_for_eur(200)}
POOL = {28_450: usdc_for_eur(28_450), 14_200: usdc_for_eur(14_200), 9_850: usdc_for_eur(9_850)}
check(OWN[500], "€500.00")
check(POOL[28_450], "€28,450.00")


def off_card(gid, home, away, start, h_odds, a_odds, league="EuroLeague"):
    """A game of a tip that is not on tonight's card (the copy page shows no price for it)."""
    return past_game(gid, home, away, start, h_odds, a_odds) | {"league": league}


g_efs = off_card("el-efs-mac-2026-10-30", team("Anadolu Efes", "EFS"), team("Maccabi", "MAC"), ms(30, 18, 0), 2.05, 1.80)
g_fkb = off_card("el-fkb-zal-2026-10-30", team("Fenerbahçe", "FKB"), team("Žalgiris", "ZAL"), ms(30, 19, 0), 2.05, 1.80)
g_mil = off_card("el-mil-par-2026-10-31", team("Milan", "MIL"), team("Partizan", "PAR"), ms(31, 18, 0), 1.75, 2.05)
g_bkn = off_card("el-bkn-vir-2026-10-31", team("Baskonia", "BKN"), team("Virtus", "VIR"), ms(31, 20, 0), 2.10, 1.70)
g_bar = off_card("el-bar-asv-2026-11-01", team("Barcelona", "BAR"), team("ASVEL", "ASV"), ms(31, 20, 45), 1.40, 2.90)
g_mon = off_card("el-mon-lyo-2026-11-01", team("Monaco", "MON"), team("Lyon", "LYO"), ms(31, 21, 0), 1.48, 2.70)
# EuroLeague Lock 3.80 (two overlapping legs), Weekend Hail Mary 6.20 (four): the last leg's price
# solved for the total at the tip's stake.
_fkb, _efs = solve_two(usdc_for_eur(350), [g_fkb["start_ms"], g_efs["start_ms"]], [], 3.80, 2.05, 2.05)
tune(g_fkb, 0, _fkb)
tune(g_efs, 0, _efs)
_bar, _mon = solve_two(usdc_for_eur(200), [g["start_ms"] for g in (g_mil, g_bkn, g_bar, g_mon)],
                       [pick_of(g, 0)["price_u"] for g in (g_mil, g_bkn)], 6.20, 1.40, 1.48)
tune(g_bar, 0, _bar)
tune(g_mon, 0, _mon)


def tip(title, legs, own, posted, pool, copies, teams, minted, exact=True):
    t = ticket(b58_id(), LORENZO, legs, OWN[own], posted, {"available": True, "c_u": OWN[own], "reason": None},
               minted, exact, tipster=LORENZO, title=title)
    t.update({"pool_u": POOL[pool], "copies": copies, "teams": teams})
    return t


MASTER = tip("Master Treble",
             [ticket_leg(0, g_rma, pick_of(g_rma, 0), "open"), ticket_leg(1, g_bos, pick_of(g_bos, 0), "open"),
              ticket_leg(2, g_ars, pick_of(g_ars, 0), "open")],
             500, 4.55, 28_450, 1284, ["RM", "BOS", "ARS"], AS_OF // 1000 - 7200,
             # Tonight's prices: Total 4.5504, so EUR 500 pays EUR 2,275.19 (no screen shows a
             # tip's own payout; the copy's EUR 25 pays exactly EUR 113.75).
             exact=False)
LOCK = tip("EuroLeague Lock",
           [ticket_leg(0, g_fkb, pick_of(g_fkb, 0), "open"), ticket_leg(1, g_efs, pick_of(g_efs, 0), "open")],
           350, 3.80, 14_200, 842, ["FKB", "EFS"], AS_OF // 1000 - 5400)
HAIL = tip("Weekend Hail Mary",
           [ticket_leg(0, g_mil, pick_of(g_mil, 0), "open"), ticket_leg(1, g_bkn, pick_of(g_bkn, 0), "open"),
            ticket_leg(2, g_bar, pick_of(g_bar, 0), "open"), ticket_leg(3, g_mon, pick_of(g_mon, 0), "open")],
           200, 6.20, 9_850, 415, ["MIL", "BKN", "BAR"], AS_OF // 1000 - 3600)

COPY_QUOTE = quote([(g, pick_of(g, 0)) for g in (g_rma, g_bos, g_ars)], QUOTE_STAKE, 4.55,
                   copy_fee_u=QUOTE_STAKE // 50)   # V-4: 2% of stake
assert eur_text(COPY_QUOTE["payout_u"]) == "€113.75"


def row(wallet, handle, roi30, copiers, settled_n, won, roi, skin, verified=True, open_count=0, staked=0):
    return {
        "wallet": wallet, "settled": settled_n, "won": won,
        "hit_rate_ppm": won * 1_000_000 // max(settled_n, 1), "roi_ppm": roi,
        "brier_ppm": 201_000, "edge_ppm": 41_000, "avg_odds_u": 3_100_000, "skin_u": skin,
        "followers_roi_ppm": 96_000, "streak": 32, "open_count": open_count,
        "open_staked_u": staked, "copies": copiers + 112,
        "fees_earned_u": 412_000_000, "handle": handle, "verified": verified,
        "roi_30d_ppm": roi30, "copiers": copiers,
    }


# 30D ROI +24.8%, win rate 76%, 1,284 copiers.
R_LORENZO = row(LORENZO, "lorenzo_hoops", 248_000, 1284, 50, 38, 152_000, 3_120_000_000,
                open_count=3, staked=OWN[500] + OWN[350] + OWN[200])
R_GIULIA = row(GIULIA, "giulia_moneyline", 112_000, 655, 33, 17, 87_000, 1_480_000_000)
R_MARCO = row(MARCO, None, 31_000, 12, 6, 2, 22_000, 64_000_000)
TIPSTERS = {"_fixture": NOTE, "as_of_ms": AS_OF, "tipsters": [R_LORENZO, R_GIULIA, R_MARCO]}
history = []
roi = 0
for i in range(12):
    roi += rng.randint(-40_000, 90_000)
    history.append({"ticket": b58_id(), "settled_at": AS_OF // 1000 - (12 - i) * 86_400,
                    "roi_cum_ppm": roi, "brier_cum_ppm": 200_000 + rng.randint(-15_000, 15_000)})
TIPSTER = {"_fixture": NOTE, "tipster": R_LORENZO, "history": history, "settled": [],
           "open": [MASTER, LOCK, HAIL]}

# GET /tipsters/{wallet}/copiers?ticket=<Master Treble>: newest first, as the drawer lists them.
COPIERS = {"_fixture": NOTE, "ticket": MASTER["ticket"], "pool_u": MASTER["pool_u"], "copiers": [
    {"wallet": b58_id(), "handle": "sol_bull99", "stake_u": usdc_for_eur(1200), "tx": sig()},
    {"wallet": b58_id("7xK9", "94Lp"), "handle": None, "stake_u": usdc_for_eur(500), "tx": sig()},
    {"wallet": b58_id(), "handle": "degencall_sol", "stake_u": usdc_for_eur(350), "tx": sig()},
    {"wallet": b58_id(), "handle": "elon_parlay", "stake_u": usdc_for_eur(250), "tx": sig()},
    {"wallet": b58_id("3bF8", "12qR"), "handle": None, "stake_u": usdc_for_eur(150), "tx": sig()},
    {"wallet": b58_id(), "handle": "ser_crypto", "stake_u": usdc_for_eur(100), "tx": sig()},
]}
check(COPIERS["copiers"][0]["stake_u"], "€1,200.00")

# ---------------------------------------------------------------- the ticker (/stream feed frames)

# "sol_bull99 placed 5.0 SOL on Real Madrid @ 1.88": 5.0 SOL at EUR 135.80 is EUR 679.00.
TICKER = [{"type": "feed", "kind": "mint", "text": "sol_bull99 placed a bet", "ts_ms": AS_OF - 20_000,
           "who": "sol_bull99", "amount_u": usdc_for_eur(679), "team": "Real Madrid", "odds_u": odds_u(1.88)}]
assert round(eur_u(TICKER[0]["amount_u"]) / SOL_PPM, 1) == 5.0

# ---------------------------------------------------------------- sentiment (event detail)


def sentiment(g, away_eur, home_eur, traders, recent):
    away_u, home_u = usdc_for_eur(away_eur), usdc_for_eur(home_eur)
    h = g["picks"][0]["price_u"]
    return {
        "game_id": g["game_id"], "symbol": g["symbol"],
        "price": {"away_u": 1_000_000 - h, "home_u": h},
        "pascal_holders": {"away": {"accounts": 41, "stake_u": away_u // 9},
                           "home": {"accounts": 63, "stake_u": home_u // 9},
                           "excluded_makers": 3, "trades_seen": 1874, "synced": True, "thin": False},
        "flow_24h": {"away_u": away_u // 4, "home_u": home_u // 4},
        "schedina": {"away_u": 120_000_000, "home_u": 310_000_000, "tickets": 18},
        "traded_all_time": {"away_u": away_u, "home_u": home_u, "total_u": away_u + home_u},
        "traders": traders,
        "recent": recent,
    }


# Fenerbahçe (away) EUR 80,700 = 41%, Panathinaikos (home) EUR 131,700 = 59%; 1,842 traders;
# "EUR 500 on Panathinaikos, 1 min ago", "EUR 250 on Fenerbahçe, 3 mins ago".
SENTIMENT = {"_fixture": NOTE, "as_of_ms": AS_OF, "min_holders": 3, "games": [
    sentiment(g_pao, 80_700, 131_700, 1842, [
        {"amount_u": usdc_for_eur(500), "team": "home", "ts_ms": AS_OF - 60_000, "source": "schedina"},
        {"amount_u": usdc_for_eur(250), "team": "away", "ts_ms": AS_OF - 180_000, "source": "pascal"}]),
    sentiment(g_rma, 31_900, 54_200, 611, []),
    sentiment(g_bos, 140_300, 96_800, 2_204, []),
]}
t = SENTIMENT["games"][0]["traded_all_time"]
check(t["away_u"], "€80,700.00")
check(t["home_u"], "€131,700.00")
assert eur_text(t["total_u"]).startswith("€212,4"), eur_text(t["total_u"])

# ---------------------------------------------------------------- fx, profile, wallet, tokens

FX = {"_fixture": NOTE, "eur_per_usd_ppm": FX_PPM, "as_of": "2026-10-29", "source": "ecb",
      "fallback": False, "eur_per_sol_ppm": SOL_PPM, "sol_as_of_ms": AS_OF, "sol_source": "pyth",
      "sol_fallback": False}
# The tipster's profile (the demo desk answers it for its own wallet, "no handle" for others).
PROFILE = {"_fixture": NOTE, "wallet": LORENZO, "handle": "lorenzo_hoops", "since": AS_OF // 1000 - 90 * 86_400,
           "verified": True,
           "links": {"x": "lorenzo_hoops", "youtube": "lorenzo_hoops", "telegram": "lorenzo_hoops",
                     "instagram": "lorenzo_hoops"},
           "followers": 12, "following": 3}
WALLET = {"_fixture": NOTE, "usdc_u": BALANCE}
TOKENS = {"_fixture": NOTE, "tokens": [
    {"mint": "F6BLLBdH7b4QiCJWibUYvws8fK9upDMP5unCCaqvgH2N", "symbol": "mockUSDC", "decimals": 6,
     "usdc_mint": True}]}

# The sportsbook's "Upcoming matches" in the mockup's order (the hero is the live game).
HOME = {"_fixture": NOTE, "mobile_order": [g["game_id"] for g in (g_rma, g_bos, g_ars)]}

FILES = {
    "home.json": HOME,
    "card.json": CARD, "quote.json": QUOTE, "slip.json": SLIP, "tickets.json": TICKETS,
    "ticket.json": PLACED, "cashout_quote.json": CASHOUT_QUOTE, "tipsters.json": TIPSTERS,
    "tipster.json": TIPSTER, "copiers.json": COPIERS, "copy_quote.json": COPY_QUOTE,
    "sentiment.json": SENTIMENT, "ticker.json": TICKER,
    "fx.json": FX, "profile.json": PROFILE, "wallet.json": WALLET, "tokens.json": TOKENS,
}
os.makedirs(OUT, exist_ok=True)
for name, body in FILES.items():
    with open(os.path.join(OUT, name), "w", encoding="utf-8") as f:
        json.dump(body, f, indent=2, ensure_ascii=False)
        f.write("\n")
print(f"wrote {len(FILES)} files to {OUT}")
