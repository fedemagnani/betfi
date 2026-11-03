// Unit check of slate.mjs (no browser, no network) on the example card (5 games) and on a
// 3-game and a 4-game slice of it, plus the phone gate's chooser on rows with a pick disabled.
// From app/: node scripts/slate_check.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { builderFeeShown, parseGateReport, refusedDuplicate, usdcFine } from "./report.mjs";
import {
  byStart, desktopReady, endOfNightLegs, gatePicks, LINE_GAME, lineLeg, MAX_RUN_ODDS_U, planTickets, PREFLIGHT_TICKET_STAKE_U,
  preflightLegs, preflightProblems, preflightTickets, prettyLabel, pegPpm, resolutionOrder, runSide, runSlate, sideOf, solFor,
  sportOf, winnerLeg,
} from "./slate.mjs";

const card = JSON.parse(readFileSync(new URL("../fixtures/desk/card.json", import.meta.url), "utf8"));
const five = byStart(card.games);
const slates = { 3: five.slice(0, 3), 4: five.slice(0, 4), 5: five };
assert.equal(five.length, 5, "the example card has 5 games");

for (const [n, games] of Object.entries(slates)) {
  const plan = planTickets(games.length);
  const all = [...plan.A, ...plan.B, ...plan.C];
  // Every leg is a game of this slate, one side per game across tickets.
  for (const [i, which] of all) {
    assert.ok(i >= 0 && i < games.length, `${n} games: leg on game ${i}`);
    assert.equal(which, sideOf(i));
    assert.ok(games[i].picks.some((p) => p.pick === which), `${n} games: game ${i} has a ${which} pick`);
  }
  for (const t of [plan.A, plan.B, plan.C]) {
    assert.equal(new Set(t.map(([i]) => i)).size, t.length, `${n} games: one leg per game`);
  }
  assert.equal(plan.A.length, 3);
  assert.equal(plan.B.length, 2);
  assert.equal(plan.C.length, 1);
  // A's first leg resolves first (the cash-out) and must leave B (the tip, copied before it) open.
  assert.ok(!plan.B.some(([i]) => i === 0), `${n} games: B avoids game 0`);
  const order = resolutionOrder(plan.A, plan.B, plan.C);
  assert.deepEqual(order[0], [0, sideOf(0)]);
  assert.equal(order.length, new Set(all.map(([i]) => i)).size, `${n} games: each game resolves once`);
  console.log(`${n} games: A ${JSON.stringify(plan.A)} B ${JSON.stringify(plan.B)} C ${JSON.stringify(plan.C)}`);
}
// The 5-game plan is 3.C's (A: games 0-2 away/home/away, B: games 3 away and 4 home).
assert.deepEqual(planTickets(5).A, [[0, "away"], [1, "home"], [2, "away"]]);
assert.deepEqual(planTickets(5).B, [[3, "away"], [4, "home"]]);
assert.deepEqual(planTickets(3).B, [[1, "home"], [2, "away"]]);
assert.throws(() => planTickets(2), /need 3/);

// Phone gate: first, second and last playable game; the 5-game choice is 3.C's picks 0, 3, 8.
const open = (n) => Array.from({ length: n }, () => ({ away: true, home: true }));
assert.deepEqual(gatePicks(open(5)).map((p) => p.nth), [0, 3, 8]);
assert.deepEqual(gatePicks(open(3)).map((p) => p.nth), [0, 3, 4]);
const rows = [{ away: false, home: false }, { away: true, home: true }, { away: true, home: false },
  { away: false, home: true }];
assert.deepEqual(gatePicks(rows).map((p) => [p.row, p.which]), [[1, "away"], [2, "away"], [3, "home"]]);
assert.throws(() => gatePicks(open(2)), /need 3/);

// The spread or total leg of ticket B (6.2a-g): on game 0, which B never holds otherwise; from
// the design card (spreads and totals as the live desk lists them, L6-2): a total first, the
// line priced nearest 50%, its long side; nothing on a winners-only card or a started game.
for (const n of [3, 4, 5]) assert.ok(!planTickets(n).B.some(([i]) => i === LINE_GAME), `${n} games: B avoids the line game`);
const design = byStart(JSON.parse(readFileSync(new URL("../fixtures/design/card.json", import.meta.url), "utf8")).games);
const withLines = design.filter((g) => lineLeg(g));
assert.ok(withLines.length >= 2, "the design card has games with lines");
for (const g of withLines) {
  const l = lineLeg(g);
  const m = g.markets.find((x) => x.picks.some((p) => p.symbol === l.symbol && p.side === l.side));
  assert.equal(l.side, "long");
  assert.equal(m.kind, l.kind);
  assert.ok(l.kind === "total" || !g.markets.some((x) => x.kind === "total" && x.picks.length === 2), "a total when there is one");
  for (const o of g.markets.filter((x) => x.kind === l.kind)) {
    const p = o.picks.find((x) => x.side === "long");
    assert.ok(Math.abs(p.price_u - 500_000) >= Math.abs(m.picks.find((x) => x.side === "long").price_u - 500_000));
  }
}
const spreadOnly = { in_play: false, markets: [
  { kind: "winner", line: null, picks: [{ label: "A", side: "long", symbol: "W", price_u: 600_000, odds_u: 1_600_000 }, { label: "B", side: "short", symbol: "W", price_u: 400_000, odds_u: 2_400_000 }] },
  { kind: "spread", line: "-3.5", picks: [{ label: "A -3.5", side: "long", symbol: "S3", price_u: 560_000, odds_u: 1_700_000 }, { label: "B +3.5", side: "short", symbol: "S3", price_u: 440_000, odds_u: 2_100_000 }] },
  { kind: "spread", line: "-4.5", picks: [{ label: "A -4.5", side: "long", symbol: "S4", price_u: 490_000, odds_u: 1_950_000 }, { label: "B +4.5", side: "short", symbol: "S4", price_u: 510_000, odds_u: 1_850_000 }] },
] };
assert.deepEqual(lineLeg(spreadOnly), { kind: "spread", line: "-4.5", label: "A -4.5", symbol: "S4", side: "long", odds_u: 1_950_000 });
assert.equal(prettyLabel(lineLeg(spreadOnly).label), "A −4.5");
assert.equal(lineLeg({ ...spreadOnly, in_play: true }), null, "lines close at tip-off");
assert.equal(lineLeg(five[0]), null, "the demo card (a replay night) lists winners only");

// Winner legs on a mixed slate (7.2a-g): an NBA game's pick is its own symbol, home short / away
// long (legacy `picks`, and `markets[0]` since L7-1); a soccer game's three outcomes are each a
// long on their own symbol (D-461), its legacy `picks` empty on a live desk (D-462). The sport
// tab a game sits under: `sport`, else basketball (a pre-7.0b desk).
{
  for (const g of five) {
    for (const which of ["home", "away"]) {
      assert.deepEqual(winnerLeg(g, which), { symbol: g.symbol, side: g.picks.find((p) => p.pick === which).side, label: g[which].name });
    }
    assert.equal(winnerLeg(g, "draw"), null, "no draw in basketball");
    assert.equal(sportOf(g), "basketball");
  }
  const ars = design.find((g) => g.sport === "soccer");
  assert.equal(sportOf(ars), "soccer");
  const live = { ...ars, picks: [] }; // as a live desk sends it
  for (const g of [ars, live]) {
    assert.deepEqual(winnerLeg(g, "home"), { symbol: "UCL_26OCT29_ARSBAY_ML.ARS", side: "long", label: "Arsenal" });
    assert.equal(winnerLeg(g, "draw").symbol, "UCL_26OCT29_ARSBAY_ML.DRAW");
    assert.equal(winnerLeg(g, "away").symbol, "UCL_26OCT29_ARSBAY_ML.BAY");
  }
  // The contract's draw (`team: "draw"`) and the design data's (`team: null`, D-455) alike.
  const contractDraw = { ...live, markets: [{ ...live.markets[0], picks: live.markets[0].picks.map((p) => (p.outcome === "X" ? { ...p, team: "draw" } : p)) }] };
  assert.equal(winnerLeg(contractDraw, "draw").symbol, "UCL_26OCT29_ARSBAY_ML.DRAW");
  assert.equal(winnerLeg(contractDraw, "home").symbol, "UCL_26OCT29_ARSBAY_ML.ARS");
  for (const g of design.filter((x) => x.sport === "basketball")) {
    assert.equal(winnerLeg(g, "home").symbol, g.symbol);
    assert.equal(winnerLeg(g, "home").side, g.picks.find((p) => p.pick === "home").side);
  }
  // The devnet slate (runSlate): unstarted games; one sport stays by start; with both, the first
  // soccer game at index 1 (ticket A plays it; game 0 keeps B's line), other soccer games last.
  assert.deepEqual(runSlate(five), five);
  assert.deepEqual(runSlate([...five].reverse().map((g, i) => ({ ...g, in_play: i === 0 }))).map((g) => g.game_id),
    five.slice(0, 4).map((g) => g.game_id), "started games are left out");
  const epl = (k, start) => ({ ...live, game_id: `epl-${k}`, symbol: `EPL_${k}`, start_ms: start });
  const mixed = [...five, epl("a", five[0].start_ms - 1), epl("b", five[4].start_ms + 1)];
  const run = runSlate(mixed);
  assert.deepEqual(run.map((g) => g.game_id), [five[0], epl("a"), five[1], five[2], five[3], five[4], epl("b")].map((g) => g.game_id));
  assert.equal(sportOf(run[LINE_GAME]), "basketball", "game 0 is basketball (B's line)");
  assert.ok(planTickets(run.length).A.some(([i]) => sportOf(run[i]) === "soccer"), "ticket A plays a soccer game");
  assert.deepEqual(runSlate(mixed, { soccer: false }), five, "E2E_SOCCER=0: basketball only");
  assert.deepEqual(runSlate([epl("a", 1), epl("b", 2), epl("c", 3)]).map((g) => g.game_id), ["epl-a", "epl-b", "epl-c"]);
  // The design build's unstarted games (by start: Real Madrid, Arsenal, Celtics) as a slate.
  const open3 = design.filter((g) => !g.in_play);
  assert.deepEqual(runSlate(design).map((g) => g.game_id), ["el-rma-oly-2026-10-29", "ucl-ars-bay-2026-10-29", "nba-bos-nyk-2026-10-29"]);
  assert.deepEqual(preflightLegs(open3, planTickets(3), null, false).map((l) => [l.symbol, l.side]), [
    ["EL_26OCT29_OLYRMA_ML.OLY", "long"], ["UCL_26OCT29_ARSBAY_ML.ARS", "long"], ["NBA_26OCT30_NYKBOS_ML.NYK", "long"],
  ]);
}

// The mockSOL ticket (E2E_SOL, 7.2a-g): the app's SOL for €x (tray.rs `sol_amount`, its own
// test: €25 at €135.80 → 0.1840 SOL), and the pool's distance from the real price.
{
  assert.equal(solFor(25_000_000, 135_800_000, 9), 184_000_000n);
  assert.equal(solFor(10_000_000, 135_800_000, 9), 73_600_000n, "€10 → 0.0736 SOL");
  assert.equal(solFor(10_000_000, 0, 9), 0);
  const fx = { eur_per_sol_ppm: 135_800_000, eur_per_usd_ppm: 921_068 };
  // 0.0736 SOL at $147.44 is 10.8516 USDC; a pegged pool pays that less its fee and impact.
  const fair = 0.0736 * (135.8 / 0.921068) * 1e6;
  const sq = (got) => ({ amount_in: "73600000", usdc_out_u: Math.round(got * 0.995), fee_u: Math.round(got * 0.005), min_out_u: 0 });
  assert.ok(Math.abs(pegPpm(sq(fair), 9, fx)) <= 1, "on the price");
  assert.ok(Math.abs(pegPpm(sq(fair * 0.99), 9, fx) + 10_000) <= 1, "1% under");
  assert.ok(pegPpm(sq(fair * 0.5), 9, fx) < -20_000, "an unpegged pool (half the price) fails the 2% check");
  assert.equal(pegPpm({ ...sq(fair), amount_in: "0" }, 9, fx), Infinity);
}

// devnet_e2e.mjs's preflight (6.2a-r, run 1 of 29 Oct): before any SOL moves, one /quote per game
// the run plays (each once, on its ticket side) and the line leg; a 503 "no two-sided book" stops
// the run with nothing sent.
{
  const g3 = slates[3];
  const line = { kind: "total", label: "Over 235.5", symbol: "T", side: "long" };
  const legs = preflightLegs(g3, planTickets(3), line);
  const sideIn = (g, which) => g.picks.find((p) => p.pick === which).side;
  assert.deepEqual(legs.map((l) => [l.symbol, l.side]), [
    [g3[0].symbol, sideIn(g3[0], "away")],
    [g3[1].symbol, sideIn(g3[1], "home")],
    [g3[2].symbol, sideIn(g3[2], "away")],
    ["T", "long"],
  ]);
  assert.equal(preflightLegs(g3, planTickets(3), null, false).length, 3, "no line: winners only");
  assert.equal(preflightLegs(five, planTickets(5), null, false).length, 5, "5 games: A and B cover all five");
  assert.deepEqual(preflightProblems(legs.map((l) => ({ ...l, status: 200, body: {} }))), []);
  const empty = { status: "unavailable", reason: "NBA_26OCT29_CLEATL_ML.CLE has no two-sided book right now" };
  const out = preflightProblems([{ ...legs[0], status: 503, body: empty }, { ...legs[1], status: 200, body: {} }]);
  assert.equal(out.length, 1);
  assert.match(out[0], /^game 0 away \(.+\): HTTP 503 unavailable "NBA_26OCT29_CLEATL_ML\.CLE has no two-sided book right now"$/);
  assert.match(preflightProblems([{ what: "x", status: 502, body: null }])[0], /HTTP 502 "\?"/);
  // devnet_e2e.mjs runs it before the first SOL transfer (wallet 1's funding).
  const src = readFileSync(new URL("./devnet_e2e.mjs", import.meta.url), "utf8");
  const pre = src.indexOf("preflightProblems(");
  assert.ok(pre > 0 && pre < src.indexOf("await burner(page"), "the preflight comes before wallet 1 is funded");
}

// The end of the night (6.2a-r, run 3): after games 0-2 resolved, the app's next /card had fewer
// games, and `games()[i].symbol` threw. The legs come from the slate as planned, whatever the
// live card holds by then.
{
  const g3 = slates[3];
  const plan = planTickets(3);
  const legOf = (i, which) => ({ symbol: g3[i].symbol, side: g3[i].picks.find((p) => p.pick === which).side, team: which });
  const played = [...plan.A, ...plan.B, ...plan.C].map(([i, w]) => legOf(i, w))
    .concat([{ symbol: "LINE", side: "long", team: "Over" }]);
  const order = resolutionOrder(plan.A, plan.B, plan.C);
  const later = g3.slice(1); // the live card once game 0 resolved
  assert.throws(() => order.map(([i]) => later[i].symbol), /symbol/, "indexing the later card fails as in run 3");
  assert.deepEqual(endOfNightLegs(g3, order, played).map((l) => l.symbol), order.map(([i]) => g3[i].symbol));
  assert.throws(() => endOfNightLegs(later, order, played), /no played leg on game 2 \(not on the slate\)/);
  const e2eSrc = readFileSync(new URL("./devnet_e2e.mjs", import.meta.url), "utf8");
  assert.match(e2eSrc, /slate = runSlate\(seen\.card\.games, \{ soccer: SOCCER \}\);\n\s*const plan = planTickets\(games\(\)\.length, \(i\) => runSide\(games\(\)\[i\], i\)\);/, "the slate is frozen at plan time, sides under the cap");
  assert.match(e2eSrc, /endOfNightLegs\(slate, resolutionOrder\(/, "the end of the night reads the frozen slate");
  assert.doesNotMatch(e2eSrc, /games\(\)\[i\]\.symbol/);
  // The preflight prices the same slate the run plays (the app's every-sport card, D-483).
  assert.match(e2eSrc, /desk\(`\/card\?date=\$\{process\.env\.SLATE_DATE\}`\);\n\s*const g = runSlate\(card\.games \?\? \[\], \{ soccer: SOCCER \}\);/);
}

// The v2 screens (7.2a-g): no L6 selector is left in the browser scripts (the header was
// `.k-header`, the top bar's `#mock-label` is gone with V-7, the Sports tab was "Home"), and the
// ids the mockSOL path uses exist in the app's source (no build lists mockSOL to dry-run it).
{
  const scripts = ["devnet_e2e.mjs", "e2e_dry.mjs", "e2e_ui.mjs", "phone_gate.mjs"]
    .map((f) => [f, readFileSync(new URL(`./${f}`, import.meta.url), "utf8").split("\n").filter((l) => !/^\s*\/\//.test(l)).join("\n")]);
  for (const [f, src] of scripts) {
    assert.doesNotMatch(src, /\.k-header\b|#mock-label(?!-account)|hasText: "Home"|nav\("Home"\)/, `${f} has an L6 selector`);
    // L8-4: My Bets is no tab and no screen; its bets are Account's #bets (e2e_ui.mjs `bets`).
    assert.doesNotMatch(src, /nav\(([\w.]+, )?"My [bB]ets"|#my-bets(?!-tabs)|"My [bB]ets":/, `${f} still opens the My Bets tab or screen`);
  }
  const app = (f) => readFileSync(new URL(`../src/${f}`, import.meta.url), "utf8");
  assert.match(app("ui/kit/stake.rs"), /id: "\{id\}-currency"/, "the stake block's currency pill is #<id>-currency");
  assert.match(app("ui/kit/stake.rs"), /Currency::Sol => "SOL"/, "its SOL option's value");
  assert.match(app("ui/tray.rs"), /StakeAmount \{\s*id: "stake",/, "the drawer's stake block is #stake");
  // L9: the verb is "Bet", or "Copy Schedina" in the drawer's copy mode.
  assert.match(app("ui/tray.rs"), /format!\("\{verb\} \\u\{2022\} \{\} SOL", format::token\(a, t\.decimals\)\)/, "the drawer's CTA in SOL");
  assert.match(app("ui/tray.rs"), /let verb = if copy\.is_some\(\) \{\s*"Copy Schedina"\s*\} else \{\s*"Bet"\s*\};/, "the drawer's verb");
  assert.match(app("ui/dev_wallet.rs"), /id: "dev-mocksol"/, "mockSOL from the dev wallet page (31 Oct: Account's faucet offers USDC and BONK)");
  assert.match(app("ui/faucet.rs"), /id: "faucet-coin"/, "Account's faucet coin pill");
  assert.match(app("ui/faucet.rs"), /id: "faucet-get"/, "Account's faucet button");
  assert.match(app("ui/faucet.rs"), /"data-usdc-u":/, "Account's USDC balance for the e2e");
  assert.match(app("ui/house.rs"), /WalletCard \{\}\s*FaucetCard \{ tokens \}\s*YourBets \{\}/, "Account: the wallet, the Faucet, then the bets (L8-4)");
  assert.match(app("ui/my_schedine.rs"), /section \{ class: "acct-bets", id: "bets",/, "the bets section is #bets (/account#bets)");
  assert.match(app("config.rs"), /"https:\/\/solscan\.io\/tx\/\{\}\?cluster=devnet"/, "View Transaction on Solscan devnet (V-9)");
}

// The phone gate's desktop pass on a live desk (6.2a-r, run 2): an empty Tipsters list is
// #board-empty instead of #board (L9); the demo pass still needs the list.
assert.equal(desktopReady("/tipsters", false), "#board");
assert.equal(desktopReady("/tipsters", true), "#board, #board-empty");
const gateSrc = readFileSync(new URL("../src/ui/tipsters/board.rs", import.meta.url), "utf8");
assert.ok(gateSrc.includes('id: "board-empty"'), "tipsters/board.rs marks the empty list #board-empty");
assert.equal(desktopReady("/account", true), "#account-wallet");
assert.equal(desktopReady("/event/x", true), "#event-teams");
assert.equal(desktopReady("/tipsters/W", false), "#tipster-head");
assert.equal(desktopReady("/copy/T", false), "#parlay-head, #copy-missing");
assert.equal(desktopReady("/copy/T/event/G", false), "#event-teams");
assert.equal(desktopReady("/ticket/T", false), "#receipt");

// devnet_e2e.mjs's builder-fee check (6.2a-r, run 2): Confirm prints it in USDC (confirm.rs
// usdc_fine), as on run 2's screen: fills of 4,999 + 2,999 + 7,040 micro-USDC.
assert.equal(usdcFine(15_038), "0.0150 USDC");
assert.equal(usdcFine(5_499), "0.0054 USDC", "truncated, not rounded");
assert.equal(usdcFine(1_234_567_890), "1,234.5678 USDC");
const run2Fills = "MOCK fill #9000000000\nbuilder fee 0.0049 USDC\nMOCK fill #9000000001\nbuilder fee 0.0029 USDC\n"
  + "MOCK fill #9000000002\nbuilder fee 0.0070 USDC\nBuilder fee\n0.0150 USDC";
assert.ok(builderFeeShown(run2Fills, 4_999 + 2_999 + 7_040));
assert.ok(!builderFeeShown(run2Fills, 16_000), "another total");
assert.ok(!builderFeeShown("Builder fee\n€0.0150", 15_038), "euros are not what Confirm prints");

// devnet_e2e.mjs (5.A, run 1): the burner's key lives in wasm memory, so after /dev/wallet the
// run must move in-app (nav links); a page load drops the wallet and the next Gioca fails.
const e2e = readFileSync(new URL("./devnet_e2e.mjs", import.meta.url), "utf8")
  .split("\n").filter((l) => !/^\s*\/\//.test(l)).join("\n");
const loads = [...e2e.matchAll(/\.(goto|reload)\(([^)]*)\)/g)].map((m) => m[0]);
assert.deepEqual(loads, [".goto(`${APP}/dev/wallet`)"], `devnet_e2e page loads: ${loads}`);

// report.mjs (5.A, run 2): the phone gate's file is its JSON report plus a summary line.
const gateOut = `${JSON.stringify([{ scheme: "light", name: "card", scrollWidth: 390, ok: true }], null, 1)}\n`
  + "PHONE GATE OK (live): 1/1 screen checks passed\n";
assert.throws(() => JSON.parse(gateOut), /JSON/);
assert.deepEqual(parseGateReport(gateOut), {
  rows: [{ scheme: "light", name: "card", scrollWidth: 390, ok: true }],
  summary: ["PHONE GATE OK (live): 1/1 screen checks passed"],
});
assert.deepEqual(parseGateReport("[]"), { rows: [], summary: [] });

// report.mjs (5.A, run 2): an older desk with the same crank key raced ticket C's settle; the
// program refused the second one with TicketNotOpen (6032). Rows as devnet_e2e records them.
const CRANK = "131Hmjktmt3zTFt7MEDs4eq76fFdw1cnBk81tGqu1CGp";
const refused = { InstructionError: [1, { Custom: 6032 }] };
const txs = [
  { sig: "a", ix: "ResolveLeg", ticket: "C", signer: CRANK, err: null },
  { sig: "b", ix: "Settle", ticket: "C", signer: CRANK, err: null },
  { sig: "c", ix: "Settle", ticket: "C", signer: CRANK, err: refused },
  { sig: "d", ix: "CashOut", ticket: "A", signer: "user", err: null },
  { sig: "e", ix: "Settle", ticket: "A", signer: CRANK, err: refused },
];
assert.equal(refusedDuplicate(txs[2], txs, CRANK), true, "C's second settle after a settle");
assert.equal(refusedDuplicate(txs[4], txs, CRANK), true, "A's settle after its cash-out");
assert.equal(refusedDuplicate(txs[1], txs, CRANK), false, "a successful settle is not a duplicate");
const lone = [{ sig: "x", ix: "Settle", ticket: "B", signer: CRANK, err: refused }];
assert.equal(refusedDuplicate(lone[0], lone, CRANK), false, "no successful close: a real failure");
const other = [txs[1], { ...txs[2], err: { InstructionError: [1, { Custom: 6000 }] } }];
assert.equal(refusedDuplicate(other[1], other, CRANK), false, "another error code: a real failure");
const user = [txs[1], { ...txs[2], signer: "user" }];
assert.equal(refusedDuplicate(user[1], user, CRANK), false, "not the crank: a real failure");
const opening = [txs[1], { ...txs[2], ix: "OpenPending" }];
assert.equal(refusedDuplicate(opening[1], opening, CRANK), false, "an open_pending is never a duplicate");
// 7.2a-r: a second finalize of a Pending the first one closed (AccountNotInitialized, 3012), as
// the user's other desks with the crank key sent on 30 Oct.
const notInit = { InstructionError: [1, { Custom: 3012 }] };
const fins = [{ sig: "f1", ix: "Finalize", ticket: "A", signer: CRANK, err: null },
  { sig: "f2", ix: "Finalize", ticket: "A", signer: CRANK, err: notInit }];
assert.equal(refusedDuplicate(fins[1], fins, CRANK), true, "a refused second finalize");
assert.equal(refusedDuplicate(fins[1], [fins[1]], CRANK), false, "no successful finalize: a real failure");
assert.equal(refusedDuplicate({ ...fins[1], err: refused }, fins, CRANK), false, "another code on finalize: a real failure");
assert.equal(refusedDuplicate({ ...fins[1], ix: "Settle" }, [{ ...fins[0], ix: "Settle" }, { ...fins[1], ix: "Settle" }], CRANK), false,
  "3012 on settle: a real failure");

// 7.2a-r (D-560): the run never plays a winner priced above 3.00 when the other team is shorter,
// so ticket A stays under the 500 USDC payout cap at €10. The 30 Oct slate's first three games
// (Hornets 2.13 / Wizards 1.86, Elversberg home, Bulls 7.69 / Celtics 1.13) played away, home,
// away paid 560 USDC on €10 and the desk refused it after SOL was sent.
{
  const nba = (k, away, home) => ({ game_id: k, symbol: `NBA_${k}`, sport: "basketball", picks: [],
    markets: [{ kind: "winner", picks: [
      { team: "away", symbol: `NBA_${k}`, side: "long", odds_u: away, outcome: "2" },
      { team: "home", symbol: `NBA_${k}`, side: "short", odds_u: home, outcome: "1" }] }] });
  const soc = (k, home, draw, away) => ({ game_id: k, symbol: `S_${k}.H`, sport: "soccer", picks: [],
    markets: [{ kind: "winner", picks: [
      { team: "home", symbol: `S_${k}.H`, side: "long", odds_u: home, outcome: "1" },
      { team: "draw", symbol: `S_${k}.D`, side: "long", odds_u: draw, outcome: "X" },
      { team: "away", symbol: `S_${k}.A`, side: "long", odds_u: away, outcome: "2" }] }] });
  const night = [nba("cha", 2_132_196, 1_855_287), soc("elv", 2_400_000, 3_300_000, 2_900_000), nba("chi", 7_692_307, 1_132_075),
    nba("det", 1_360_000, 3_580_000), nba("nyk", 1_850_000, 2_130_000), soc("vil", 3_400_000, 2_900_000, 3_600_000)];
  const side = (i) => runSide(night[i], i);
  assert.deepEqual([0, 1, 2, 3, 4].map(side), ["away", "home", "home", "away", "home"], "the Bulls (7.69) give way to the Celtics");
  assert.equal(side(5), "home", "both teams above 3.00: the shorter one (never the draw)");
  assert.equal(runSide(soc("x", 4_000_000, 2_000_000, 3_500_000), 1), "away", "home 4.00 → away 3.50");
  assert.equal(runSide(nba("y", 3_500_000, 4_000_000), 0), "away", "the other side is longer: kept");
  assert.equal(runSide(five[0], 0), sideOf(0), "the example card keeps the pattern");
  const plan = planTickets(night.length, side);
  const oddsOf = (i, w) => night[i].markets[0].picks.find((p) => p.team === w).odds_u;
  for (const t of [plan.A, plan.B]) for (const [i, w] of t) assert.ok(oddsOf(i, w) <= MAX_RUN_ODDS_U, `game ${i} ${w}`);
  const prod = plan.A.reduce((x, [i, w]) => x * oddsOf(i, w) / 1e6, 1);
  assert.ok(prod * PREFLIGHT_TICKET_STAKE_U / 1e6 < 500, `A pays ${prod.toFixed(2)} × 12 USDC, under the cap`);
  assert.deepEqual(planTickets(5), planTickets(5, sideOf), "the default side is the fixed pattern");
  // The preflight asks each whole ticket too: A, B (+ the line), C.
  const line = { symbol: "TT", side: "long", kind: "total", label: "Over 1" };
  const tickets = preflightTickets(night, plan, line);
  assert.deepEqual(tickets.map((t) => t.what), ["ticket A", "ticket B", "ticket C"]);
  assert.deepEqual(tickets[0].legs, [{ symbol: "NBA_cha", side: "long" }, { symbol: "S_elv.H", side: "long" }, { symbol: "NBA_chi", side: "short" }]);
  assert.deepEqual(tickets[1].legs.at(-1), { symbol: "TT", side: "long" });
  assert.equal(preflightTickets(night, plan, null, false).length, 2, "no C: A and B");
  assert.ok(PREFLIGHT_TICKET_STAKE_U >= Math.ceil(10e6 * 1e6 / 900_000), "at least €10 at 0.90 EUR per USD");
  assert.deepEqual(preflightProblems([{ what: "ticket A", status: 422, body: { status: "rejected", reason: "the payout is above the 500.00 USDC cap; lower the stake" } }]),
    ['ticket A: HTTP 422 rejected "the payout is above the 500.00 USDC cap; lower the stake"']);
}

// 7.2a-r: the end of the night finds a soccer pick by its own outcome symbol. A soccer game's
// `symbol` is its home outcome's; attempt 5 played Mainz (away) and failed "no played leg on game
// 1 (BUND_26OCT30_ELVM05_ML.ELV)" after every ticket had landed.
{
  const soc = { game_id: "bundesliga-elv-m05", symbol: "BUND_ELVM05_ML.ELV", sport: "soccer", picks: [], markets: [{ kind: "winner", picks: [
    { team: "home", symbol: "BUND_ELVM05_ML.ELV", side: "long", odds_u: 3_280_000, outcome: "1" },
    { team: "draw", symbol: "BUND_ELVM05_ML.DRAW", side: "long", odds_u: 3_930_000, outcome: "X" },
    { team: "away", symbol: "BUND_ELVM05_ML.M05", side: "long", odds_u: 2_100_000, outcome: "2" }] }] };
  const nbaG = five[0];
  const played = [{ symbol: nbaG.symbol, side: nbaG.picks.find((p) => p.pick === "away").side }, { symbol: "BUND_ELVM05_ML.M05", side: "long" }];
  assert.deepEqual(endOfNightLegs([nbaG, soc], [[0, "away"], [1, "away"]], played).map((l) => l.symbol), [nbaG.symbol, "BUND_ELVM05_ML.M05"]);
  assert.deepEqual(endOfNightLegs([nbaG, soc], [[1, "home"]], [{ symbol: "BUND_ELVM05_ML.ELV", side: "long" }]).map((l) => l.symbol), ["BUND_ELVM05_ML.ELV"]);
  assert.throws(() => endOfNightLegs([nbaG, soc], [[1, "draw"]], played), /no played leg on game 1 \(BUND_ELVM05_ML\.DRAW\)/);
  assert.equal(runSide(soc, 1), "away", "Elversberg at 3.28 gives way to Mainz at 2.10");
}

// 7.2a-r: the cash-out signs on a root task. On a live desk the ticket closes (and its card leaves
// Open, unmounting the cash-out block) before the wallet's confirmation returns; a task of that
// block was dropped with it, so the celebration never opened (devnet attempt 4, 30 Oct).
{
  const mb = readFileSync(new URL("../src/ui/my_schedine.rs", import.meta.url), "utf8");
  const confirm = mb.slice(mb.indexOf('id: format!("cashout-confirm-'), mb.indexOf('label: format!("Confirm cash out'));
  assert.match(confirm, /onclick: move \|_\| \{\s*(\/\/[^\n]*\n\s*)*dioxus::core::spawn_forever\(sign\(/, "Confirm cash out spawns the signing on a root task");
  // 1 Nov: one tap; the offer is asked and signed on one root task.
  const ask = mb.slice(mb.indexOf("let ask = move |seen: Option<u64>|"), mb.indexOf("// The figure shown"));
  assert.match(ask, /dioxus::core::spawn_forever\(async move \{[\s\S]*sign\(offer, at\)\.await/, "the one-tap cash-out signs on a root task");
  const sign = mb.slice(mb.indexOf("let sign = move |offer: CashoutQuote"), mb.indexOf("// The figure shown"));
  assert.doesNotMatch(sign, /\b(cash|cashing|landed)\.(set|write)\(/, "the root task writes the screen's signals only while they live (set_alive, try_write)");
}

console.log("SLATE CHECK OK: 3, 4 and 5-game cards; the run slate with NBA and soccer (winner legs, sport tabs); the mockSOL amount and peg; the spread or total leg; the preflight (picks and whole tickets); the run's sides under the payout cap; the desktop pass on an empty board; the builder fee in USDC; the end of the night on the frozen slate (soccer outcomes by symbol); devnet_e2e loads a page only at /dev/wallet; no L6 selector left; the cash-out signs on a root task; the mockSOL ids in the app; run reports");
