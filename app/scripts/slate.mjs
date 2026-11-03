// Leg choosers for the browser scripts (phone_gate.mjs, devnet_e2e.mjs), for any slate of 3 or
// more games: a live night may have only 3 NBA games, so no script clicks a fixed pick index.
// Pure functions, no browser: `node scripts/slate_check.mjs` checks them on a 3-game and a
// 5-game card.

/// The pick played on the i-th game of the night (games by start time). One pick per game for
/// every ticket of a run, so resolving a game for its pick never makes another ticket lose. The
/// first five match the 5-game plan of 3.C (A: away, home, away; B: away, home).
export const SIDES = ["away", "home", "away", "away", "home"];
export const sideOf = (i) => SIDES[i] ?? (i % 2 ? "home" : "away");

/// A `/card` answer's games by start time (then symbol), as the scripts number them.
export function byStart(games) {
  return [...games].sort((a, b) => a.start_ms - b.start_ms || a.symbol.localeCompare(b.symbol));
}

/// The phone gate's three picks from the Card as shown: `rows[i] = {away, home}`, true where
/// that pick's button is enabled. Takes the first, second and last game with a playable pick
/// (away, home, away where enabled, else the other side). Returns `[{row, which, nth}]`, `nth`
/// being the `.pick` index on the page (two buttons per game).
export function gatePicks(rows) {
  const playable = rows.map((r, i) => ({ ...r, i })).filter((r) => r.away || r.home);
  if (playable.length < 3) throw new Error(`the card has ${playable.length} playable games, need 3`);
  const chosen = [playable[0], playable[1], playable[playable.length - 1]];
  return chosen.map((r, k) => {
    const want = k === 1 ? "home" : "away";
    const which = r[want] ? want : want === "away" ? "home" : "away";
    return { row: r.i, which, nth: 2 * r.i + (which === "away" ? 0 : 1) };
  });
}

/// A game's sport tab on the Sportsbook (L7-1 `/card`: `"basketball" | "soccer"`; a desk older
/// than 7.0b names none, and its games are NBA).
export const sportOf = (game) => game?.sport ?? "basketball";

/// The winner pick `which` ("home", "away" or "draw") of a `/card` game as the desk prices it:
/// `{symbol, side, label}`. A soccer game's winner lives in `markets[0]` (one long symbol per
/// outcome, D-461/D-462; its legacy `picks` is empty); an NBA game's in `markets[0]` too since
/// L7-1, with the game's own symbol, and in the legacy `picks` before it. `null` when the game
/// has no such pick.
export function winnerLeg(game, which) {
  const m = (game?.markets ?? []).find((x) => x.kind === "winner");
  const isDraw = (p) => p.team === "draw" || p.outcome === "X";
  const mp = (m?.picks ?? []).find((p) => (which === "draw" ? isDraw(p) : p.team === which && !isDraw(p)));
  if (mp?.symbol && mp?.side) return { symbol: mp.symbol, side: mp.side, label: mp.label ?? game[which]?.name ?? which };
  const p = (game?.picks ?? []).find((x) => x.pick === which);
  if (!p) return null;
  return { symbol: game.symbol, side: p.side, label: game[which]?.name ?? which };
}

/// The devnet run's slate (7.2a-g; L7-5: "NBA, plus EPL when it has games"): the card's unstarted
/// games by start time; on a card of both sports, the basketball games in that order with the
/// first soccer game moved to index 1 (so ticket A always plays one: A holds games 0-2, and game
/// 0 stays a basketball game, whose spread or total ticket B adds; soccer lists none) and the
/// other soccer games last. `soccer: false` leaves soccer out (E2E_SOCCER=0).
export function runSlate(games, { soccer = true } = {}) {
  const open = byStart(games.filter((g) => !g.in_play));
  const bb = open.filter((g) => sportOf(g) !== "soccer");
  const fb = open.filter((g) => sportOf(g) === "soccer");
  if (!soccer) return bb;
  if (!bb.length || !fb.length) return open;
  return [bb[0], fb[0], ...bb.slice(1), ...fb.slice(1)];
}

/// The mockSOL the app pays for `eurU` micro-euros at the desk's `/fx` `eur_per_sol_ppm` (tray.rs
/// `sol_amount`: base units of a `decimals`-place mint, truncated to 4 decimal places).
export function solFor(eurU, eurPerSolPpm, decimals) {
  if (!(eurPerSolPpm > 0)) return 0;
  const units = (BigInt(eurU) * 10n ** BigInt(decimals)) / BigInt(eurPerSolPpm);
  const step = 10n ** BigInt(Math.max(0, decimals - 4));
  return (units / step) * step;
}

/// How far the mockSOL pool sits from the real SOL price (V-3, `crank amm-peg`), in signed ppm:
/// what a `/swap-quote` (`sq`) pays for its `amount_in` mockSOL before the swap fee
/// (`usdc_out_u + fee_u`: the pool's price, with its price impact) over what that much SOL is
/// worth in USDC at the desk's `/fx` (`eur_per_sol_ppm ÷ eur_per_usd_ppm`).
export function pegPpm(sq, decimals, fx) {
  const fair = (Number(BigInt(sq.amount_in)) / 10 ** decimals) * (fx.eur_per_sol_ppm / fx.eur_per_usd_ppm) * 1e6;
  if (!(fair > 0)) return Infinity;
  return Math.round(((sq.usdc_out_u + sq.fee_u - fair) / fair) * 1e6);
}

/// The longest winner price the devnet run plays (7.2a-r): three legs at most 3.00 stay under the
/// desk's 500 USDC payout cap at a €10 stake (27 × 11.2 ≈ 302 USDC before the margin).
export const MAX_RUN_ODDS_U = 3_000_000;

/// The pick the devnet run plays on game `i` of `game`'s slate (7.2a-r): `sideOf(i)`, unless the
/// card prices it above `MAX_RUN_ODDS_U` and the other team shorter; then the other team (never
/// the draw). On 30 Oct the fixed pattern picked the Bulls at 7.69 and ticket A's three legs paid
/// 560 USDC on €10: the desk refused it (422, "above the 500.00 USDC cap") after SOL was sent.
export function runSide(game, i) {
  const want = sideOf(i);
  const other = want === "away" ? "home" : "away";
  const odds = (which) => {
    const m = (game?.markets ?? []).find((x) => x.kind === "winner");
    const mp = (m?.picks ?? []).find((p) => p.team === which && p.outcome !== "X");
    return mp?.odds_u ?? (game?.picks ?? []).find((p) => p.pick === which)?.odds_u ?? 0;
  };
  const [a, b] = [odds(want), odds(other)];
  return a > MAX_RUN_ODDS_U && b > 0 && b < a ? other : want;
}

/// The run's tickets as the desk will be asked to price them, before any SOL moves (7.2a-r):
/// `[{what, legs: [{symbol, side}]}]`, A, B (plus `line`) and, with `withC`, C, each to be
/// quoted at `PREFLIGHT_TICKET_STAKE_U`. A one-pick quote cannot see the payout cap.
export function preflightTickets(games, plan, line, withC = true) {
  const legsOf = (t) => t.map(([i, which]) => {
    const p = winnerLeg(games[i], which);
    if (!p) throw new Error(`game ${i} has no ${which} pick`);
    return { symbol: p.symbol, side: p.side };
  });
  const out = [{ what: "ticket A", legs: legsOf(plan.A) },
    { what: "ticket B", legs: [...legsOf(plan.B), ...(line ? [{ symbol: line.symbol, side: line.side }] : [])] }];
  if (withC) out.push({ what: "ticket C", legs: legsOf(plan.C) });
  return out;
}
/// At least the €10 preset at any EUR/USD rate above 0.834 (micro-USDC).
export const PREFLIGHT_TICKET_STAKE_U = 12_000_000;

/// The devnet run's tickets over `n` games (by start time), as `[gameIndex, which]` lists:
/// - A: games 0, 1, 2 (its first leg resolves first, for the cash-out);
/// - B: two games other than game 0, preferring games A does not hold (3 and 4 on a 5-game
///   night, 1 and 2 on a 3-game night); the tip when the tip flow is on, so its games must stay
///   open until it is copied;
/// - C: the night's last game alone (the ticket paid in mockBONK).
/// Every ticket plays the same side of a shared game (`sideOf`).
/// `side(i)` names the pick played on game i (default `sideOf`; the devnet run passes `runSide`).
export function planTickets(n, side = sideOf) {
  if (!(n >= 3)) throw new Error(`the card has ${n} games, need 3`);
  const leg = (i) => [i, side(i)];
  const A = [0, 1, 2];
  const B = [...Array.from({ length: n - 3 }, (_, k) => k + 3), 1, 2].slice(0, 2).sort((x, y) => x - y);
  const C = [n - 1];
  return { A: A.map(leg), B: B.map(leg), C: C.map(leg) };
}

/// The distinct (game, side) legs of `tickets`, first ticket's legs first: each game is resolved
/// once, for that side.
export function resolutionOrder(...tickets) {
  const seen = new Map();
  for (const t of tickets) for (const [i, which] of t) if (!seen.has(i)) seen.set(i, which);
  return [...seen.entries()];
}

/// The game whose spread or total the devnet run adds to ticket B (6.2a-g): game 0. B never
/// holds game 0's winner (A's first leg, resolved first for the cash-out), and a line is a market
/// of its own, so resolving game 0's winner leaves B open.
export const LINE_GAME = 0;

/// The spread or total leg played on `game` (a `/card` game with L6-2 `markets`): a total if the
/// game lists one, else a spread; of those lines, the one whose long side is priced nearest 50%
/// (the middle of the desk's three, D-320); its long side (Over, or the listed team's line),
/// which the run resolves at 1. `null` when the game lists no priced line (a replay tape holds
/// winners only, L6-3) or has started (lines close at tip-off, U-2).
export function lineLeg(game) {
  if (!game || game.in_play) return null;
  for (const kind of ["total", "spread"]) {
    const longs = (game.markets ?? []).filter((m) => m.kind === kind && (m.picks ?? []).length === 2)
      .map((m) => ({ m, p: m.picks.find((p) => p.side === "long" && p.odds_u > 0 && p.price_u > 0) }))
      .filter((x) => x.p);
    if (!longs.length) continue;
    const near = (x) => Math.abs(x.p.price_u - 500_000);
    const { m, p } = longs.reduce((a, b) => (near(b) < near(a) ? b : a));
    return { kind, line: m.line, label: p.label, symbol: p.symbol, side: p.side, odds_u: p.odds_u };
  }
  return null;
}

/// A market label as the app prints it: a typographic minus before a line ("Real Madrid −3.5").
export const prettyLabel = (label) => label.replace(/ -/g, " −");

/// The one-pick `/quote` requests that tell, before any SOL moves, whether the desk can price
/// every pick the devnet run plays now (6.2a-r): each game of `plan` once (its ticket side, as
/// `/card` names it) and the `line` leg. On 29 Oct at 18:40 UTC Pascal's NBA game books were
/// empty while `/card` still listed odds (from the marks): every `/quote` answered 503 "no
/// two-sided book", and the run had already funded a burner. `withC`: ticket C is played.
export function preflightLegs(games, plan, line, withC = true) {
  const legs = [];
  for (const [i, which] of resolutionOrder(plan.A, plan.B, withC ? plan.C : [])) {
    const g = games[i];
    const p = winnerLeg(g, which);
    if (!p) throw new Error(`game ${i} has no ${which} pick`);
    legs.push({ what: `game ${i} ${which} (${g[which]?.name ?? g.symbol})`, symbol: p.symbol, side: p.side });
  }
  if (line) legs.push({ what: `game ${LINE_GAME} ${line.kind} "${line.label}"`, symbol: line.symbol, side: line.side });
  return legs;
}

/// The played legs to resolve at the end of the night, one per game of `order` (`[gameIndex,
/// which]`, from `resolutionOrder`), found in `legs` by the symbol of `slate[gameIndex]`. `slate`
/// is the games as planned, frozen then: a resolved game leaves the desk's /card, so the indexes
/// do not hold on a later card (6.2a-r run 3 failed on that).
export function endOfNightLegs(slate, order, legs) {
  return order.map(([i, which]) => {
    // A soccer outcome is its own symbol (D-461): the game's `symbol` names its home outcome
    // only, so the played pick's symbol is looked up (7.2a-r: Mainz away on 30 Oct).
    const sym = (which && winnerLeg(slate[i], which)?.symbol) || slate[i]?.symbol;
    const l = legs.find((x) => x.symbol === sym);
    if (!l) throw new Error(`no played leg on game ${i} (${sym ?? "not on the slate"})`);
    return l;
  });
}

/// The phone gate's desktop pass: the element that says screen `path` has drawn. On a live desk
/// (`live`) the Tipsters board may hold nobody yet (5 settled tips and 20 USDC of own stake in
/// 30 days; 29 Oct, run 2 of 6.2a-r: nobody qualified), and then the screen says so instead of
/// drawing `#board`; the demo fixtures always fill it.
export function desktopReady(path, live) {
  const fixed = { "/account": "#account-wallet" }[path];
  if (fixed) return fixed;
  if (path === "/tipsters") return live ? "#board, #board-empty" : "#board";
  if (path.startsWith("/event/")) return "#event-teams";
  if (path.startsWith("/tipsters/")) return "#tipster-head";
  if (/^\/copy\/[^/]+\/event\//.test(path)) return "#event-teams";
  if (path.startsWith("/copy/")) return "#parlay-head, #copy-missing";
  return "#receipt";
}

/// From `[{what, status, body}]` (the preflight's `/quote` answers): one line per pick the desk
/// would not price, with its reason; empty when every pick was priced (HTTP 200).
export function preflightProblems(answers) {
  return answers.filter((a) => a.status !== 200)
    .map((a) => `${a.what}: HTTP ${a.status} ${a.body?.status ?? ""} "${a.body?.reason ?? "?"}"`.replace(/ {2,}/g, " "));
}
