// Plan 3.C, extended for L5 (5.0b), the L6 screens (6.2a-g) and the v2 screens (7.2a-g): the whole
// schedina lifecycle on devnet, driven only through the app's UI in a headless Chromium at 390 px
// (dark, the app's only theme, D-310). Started by scripts/demo_devnet.sh --e2e, which runs the
// mock, the desk (demo profile) and the app, and passes:
//   APP_URL DESK_URL MOCK_URL RPC_URL FUNDER_KEYPAIR FUND_SOL OUT CRANK_BIN PLAYWRIGHT_CORE [CHROME_PATH]
//
// Any slate of 3 or more unstarted games: the legs come from the desk's /card (as the app reads
// it: every sport, D-483) through scripts/slate.mjs `runSlate`, never from fixed pick positions
// (by start time; NBA plus EPL when the card has both: the first soccer game is game 1, so A plays
// it; A: games 0-2; B: two games other than game 0, games 3 and 4 when there are 5; C: the last
// game; every ticket plays the same side of a shared game, and each game is resolved once). Each
// pick is pressed on its sport's tab (7.1a, D-480); a soccer pick is a long on its outcome's own
// symbol (D-461). E2E_SOCCER=0 keeps the slate to basketball.
//
// Flow: /dev/wallet → a fresh in-page burner wallet → FUND_SOL devnet SOL from the funder by
// `solana transfer` (the only step outside the browser besides the mock clock) → Account →
// the faucet's USDC airdrop (#faucet-get, its celebration; the balance on Account) →
// ticket A, 3 picks on the Sportsbook → the slip pill (#slip-bar "3 Picks • Total X") opens the
// bet drawer (7.1b, a BottomSheet: €10 from #stake-presets, "Bet • €10.00"; its Total,
// #slip-total; a worse price's #price-moved banner accepted) → Bet (#gioca: escrowed → hedging
// in the CTA) → minted: the placement celebration on /confirm (#celebration: View Transaction =
// the open_pending on Solscan) → ticket B, 2 picks plus a spread or total on
// game 0 from its event page (slate.mjs `lineLeg`: a total first, its Over; skipped with a note
// when the card lists no line, as on a replay tape) → minted → [L5/L7 parts below] → leg 1 of A
// resolves on the mock (/mock/resolve) → the desk rolls → Cash out in Account's bets (#bets: offer, then sign)
// → the cash-out celebration (View Transaction = the cash_out on Solscan) → cashed out → B's legs
// (the line too) resolve → B settles → every other game resolves; no ticket stays open.
//
// Rehearsal parts (5.A, 7.2a-r turn them on), each behind its flag:
//   E2E_TIP=1   B is shared as a tip (the drawer's tip toggle, #share-tip): the desk lists it
//               with tipster = the wallet; its settle passes the tipster profile.
//   E2E_COPY=1  (needs E2E_TIP) a second burner wallet, funded with FUND2_SOL (default 0.008)
//               from the same FUNDER_KEYPAIR, gets test USDC and copies B the way a follower
//               does (L9): Tipsters → the tipster's page → B's parlay (/copy/<B>, its games locked)
//               → Copy Schedina, the bet drawer in copy mode (the €10 preset, "Copy Schedina •
//               €10.00") → the celebration on /confirm (View Transaction on Solscan). The copy fee
//               is the quote's ceil(2% of the stake) for the stake the drawer priced, the copy is
//               minted with copied_from = B, and its finalize pays the tipster's USDC account (the
//               desk's fees_earned_u says the same). The copy settles with B.
//   E2E_BONK=1  ticket C (the last game) paid in mockBONK: BONK in the drawer's coin selector
//               (#stake-currency), "Get test mockBONK" (#token-faucet, the AMM's faucet), BONK_AMOUNT
//               (default 10000) mockBONK typed in #token-amount, the swap fee line, stake = the
//               swap quote's min_out_u (the /quote asked and the ticket on chain), one [swap,
//               verify, open_pending] transaction.
//   E2E_SOL=1   ticket E (the last game, as C) paid in mockSOL through the stake block's currency
//               pill (#stake-currency "SOL", V-3): 1 test mockSOL from Account's DEPOSIT
//               (#dev-mocksol on /dev/wallet), then €10 on the presets: the app pays €10 ÷ the desk's
//               EUR/SOL in mockSOL ("Bet • x SOL"). Needs the mockSOL pool pegged to the real SOL
//               price first (`crank amm-peg --from-fx`, which demo_devnet.sh runs with E2E_SOL):
//               the swap must turn that mockSOL into €10 of USDC within 2% (the 1% peg tolerance,
//               the 0.5% swap fee, price impact), then the same checks as C.
// FUND_SOL + FUND2_SOL (when E2E_COPY) must stay within 0.05 SOL.
//
// Then the evidence: every devnet signature of the run (wallets, tickets) classified by its
// program log, and every number the screens showed checked against the desk's /quote,
// /copy-quote, /swap-quote and /cashout-quote answers and the transactions' token balance
// changes. Every screenshot is also checked as the phone gate checks a screen (screen_checks.mjs:
// dark, no horizontal scroll, no trading words, values toned, figures in tabular Plus Jakarta
// Sans; the test-money marker: the TEST chip on screens with the app bar, the celebrations'
// "mock-filled" line, the honesty pill on Account, V-8/D-311). Screenshots → OUT/*.png,
// evidence → OUT/evidence.json and OUT/evidence.md. Exits 1 on any failed check. The RPC URL (it
// may carry an API key) is never printed.
//
// Selectors it depends on (ids and routes; e2e_ui.mjs holds the shared steps, e2e_dry.mjs runs
// them on local builds): #bottom-nav a[href], .k-subheader-start .k-icon-btn, #sport-<sport>,
// #burner, #wallet-address, #sol-refresh, #sol-balance, #account-address[title], #faucet-coin, #faucet-get,
// #faucet-celebration (#celebrate-tx, #faucet-close), #dev-mocksol, #test-result, #account-balance[data-usdc-u], .home-game[data-game-id]
// .pick-home/.pick-away, the event link a[href$="/event/<id>"], #event-tab-spread/-total,
// #event-lines .ev-pick[aria-label], #slip-bar (.k-slip-pill-count, .k-slip-pill-total b),
// #bet-drawer, #slip-picks .k-pick, #stake-presets, #stake-currency, #slip-total,
// #price-moved .k-moved-accept, #share-tip, #tip-why, #swap-stake, #token-faucet, #token-note,
// #token-line, #token-amount, #swap-fee, #gioca, #submit-error, #celebration (#celebrate-slip,
// #celebrate-tx, #celebrate-brag, #celebrate-total .k-stat-value, #celebrate-amount
// .k-payout-eur), li.ticket#ticket-<pda>, .mb-cashout, .mb-offer[data-c-u],
// .mb-cashout-confirm, .mb-cash-eur/-usdc, .mb-leg[data-status], #cashout-celebration
// (#celebrate-amount, #celebrate-tx, #cashout-close),
// #bets #my-bets-tabs, .mb-result, .mb-paid[data-paid-u], .closing a, #board a.tp-row-link,
// #open-tips [data-ticket] a.tp-parlay, #parlay-head, #parlay-legs .pl-leg, #tip-copy,
// #tip-copy-why, #bet-drawer #slip-copy-fee, #gioca, #celebration #celebrate-tx,
// #account-house #house-hedger, .k-appbar
// #balance (euros only), .k-appbar .wallet, #account-faucet .acct-test-chip.
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { refusedDuplicate, usdcFine } from "./report.mjs";
import * as ui from "./e2e_ui.mjs";
import { problems, screenFacts, settle } from "./screen_checks.mjs";
import {
  endOfNightLegs, LINE_GAME, lineLeg, pegPpm, planTickets, PREFLIGHT_TICKET_STAKE_U, preflightLegs, preflightProblems,
  preflightTickets, prettyLabel, resolutionOrder, runSide, runSlate, solFor, winnerLeg,
} from "./slate.mjs";

const env = (k, d) => {
  const v = process.env[k] ?? d;
  if (v === undefined) throw new Error(`${k} is not set`);
  return v;
};
const flag = (k) => /^(1|true|yes)$/i.test(process.env[k] ?? "");
const APP = env("APP_URL");
const DESK = env("DESK_URL");
const MOCK = env("MOCK_URL");
const RPC = env("RPC_URL");
const FUNDER = env("FUNDER_KEYPAIR");
const FUND_SOL = env("FUND_SOL", "0.005");
const OUT = env("OUT");
const USDC_MINT = "F6BLLBdH7b4QiCJWibUYvws8fK9upDMP5unCCaqvgH2N";
const CRANK = env("CRANK_PUBKEY", "131Hmjktmt3zTFt7MEDs4eq76fFdw1cnBk81tGqu1CGp");
const CLUSTER = env("CLUSTER", "devnet"); // localnet = the free rehearsal (demo_devnet.sh --localnet)
const PROGRAM = "HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW";
const TIP = flag("E2E_TIP");
const COPY = flag("E2E_COPY");
const BONK = flag("E2E_BONK");
const SOL = flag("E2E_SOL");
// The slate keeps soccer unless E2E_SOCCER is 0/false/no (runSlate).
const SOCCER = !/^(0|false|no)$/i.test(process.env.E2E_SOCCER ?? "");
// Ticket C (mockBONK) and E (mockSOL) play the night's last game.
const LAST = BONK || SOL;
const FUND2_SOL = env("FUND2_SOL", "0.008");
const BONK_AMOUNT = env("BONK_AMOUNT", "10000");
const TIPSTER_SHARE_BPS = Number(env("TIPSTER_SHARE_BPS", "8000"));
if (/mainnet/i.test(RPC)) throw new Error("mainnet RPC refused");
// Devnet: the public endpoint or a hosted devnet one (D-131); localnet: 127.0.0.1 only.
const rpcOk = (() => {
  try {
    const u = new URL(RPC);
    return CLUSTER === "devnet"
      ? u.protocol === "https:" && /(^|[.-])devnet([.-]|$)/.test(u.hostname)
      : u.protocol === "http:" && u.hostname === "127.0.0.1";
  } catch { return false; }
})();
if (!rpcOk) throw new Error(`unexpected RPC for ${CLUSTER} (not shown)`);
if (/pascal\.trade/i.test(MOCK + DESK)) throw new Error("mode M: no Pascal host");
const solTotal = Number(FUND_SOL) + (COPY ? Number(FUND2_SOL) : 0);
if (!(Number(FUND_SOL) > 0 && (!COPY || Number(FUND2_SOL) > 0) && solTotal <= 0.05 + 1e-12)) {
  throw new Error(`FUND_SOL${COPY ? " + FUND2_SOL" : ""} must be in (0, 0.05] (is ${solTotal})`);
}
if (COPY && !TIP) throw new Error("E2E_COPY copies the run's tip: set E2E_TIP=1 too");

const { chromium } = await import(env("PLAYWRIGHT_CORE", "playwright-core"));
const CHROME = process.env.CHROME_PATH
  ?? `${homedir()}/Library/Caches/ms-playwright/chromium_headless_shell-1208/chrome-headless-shell-mac-arm64/chrome-headless-shell`;

const t0 = Date.now();
const say = (s) => console.log(`[${((Date.now() - t0) / 1000).toFixed(1).padStart(6)} s] ${s}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const checks = [];
const check = (what, ok, detail = "") => {
  checks.push({ what, ok: !!ok, detail });
  say(`${ok ? "ok  " : "FAIL"} ${what}${detail ? ` (${detail})` : ""}`);
};
const onOff = (b) => (b ? "on" : "off");
say(`flags: tip ${onOff(TIP)}, copy ${onOff(COPY)}, mockBONK ${onOff(BONK)}, mockSOL ${onOff(SOL)}, soccer ${onOff(SOCCER)}; SOL ${solTotal}`);

// --- the app's formatting (app/src/format.rs), to compare screens with the desk's integers ---
const group = (n) => n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
const eur = (u) => `€${group(Math.floor(u / 1e6))}.${String(Math.floor((u % 1e6) / 1e4)).padStart(2, "0")}`;
// The USDC line under a euro figure (kit::money::usdc_text), and the amounts of the slip's fee
// details and Confirm (6.1c): "4.65 USDC".
const usdc = (u) => `${eur(u).slice(1)} USDC`;
const odds = (u) => `${Math.floor(u / 1e6)}.${String(Math.floor((u % 1e6) / 1e4)).padStart(2, "0")}`;
const copyFee = (stake) => Number((BigInt(stake) * 200n + 9_999n) / 10_000n); // ceil(2%), D-064
// A typed token amount as format::token shows it: whole part grouped, no trailing zeros.
const tokenText = (s) => {
  const [whole, frac = ""] = s.replace(/,/g, "").trim().split(".");
  const f = frac.replace(/0+$/, "");
  return `${group(BigInt(whole || "0"))}${f ? `.${f}` : ""}`;
};

// --- devnet JSON-RPC, polite on the public endpoint ---
async function rpc(method, params) {
  for (let i = 0, wait = 1000; ; i++, wait *= 2) {
    const r = await fetch(RPC, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
    });
    if (r.status === 429 && i < 6) { await sleep(wait); continue; }
    const j = await r.json();
    if (j.error) throw new Error(`${method}: ${JSON.stringify(j.error)}`);
    return j.result;
  }
}
const desk = async (path) => (await fetch(DESK + path)).json();

// --- preflight (6.2a-r): before any SOL moves, the desk must price every pick the run plays.
// /card lists odds from the marks even when Pascal has no two-sided book (29 Oct, 18:40 UTC: the
// NBA game books were empty upstream), and then every /quote answers 503 after a burner is
// funded. One /quote per game (its ticket side) and the line leg, at the minimum stake; nothing
// is signed or sent. The card is the one the app reads (every sport, D-483) and the slate the
// run's (runSlate). SLATE_DATE comes from demo_devnet.sh. With E2E_SOL, the desk must list
// mockSOL and the pool must be pegged (within 2% for €10, `crank amm-peg --from-fx`).
const PEG_PPM = 20_000;
const SOL_EUR_U = 10_000_000; // the €10 preset
const solToken = (tokens) => (tokens?.tokens ?? []).find((t) => !t.usdc_mint && /SOL/.test(t.symbol));
async function solPeg() {
  const tok = solToken(await desk("/tokens"));
  if (!tok) return { problem: "the desk's /tokens lists no mockSOL (MOCK_AMM_PROGRAM_ID, amm-setup?)" };
  const fx = await desk("/fx");
  const amount = solFor(SOL_EUR_U, fx.eur_per_sol_ppm ?? 0, tok.decimals);
  if (!amount) return { problem: "the desk's /fx has no eur_per_sol_ppm" };
  const r = await fetch(`${DESK}/swap-quote`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ mint: tok.mint, amount_in: String(amount) }),
  });
  if (r.status !== 200) return { problem: `/swap-quote mockSOL: HTTP ${r.status}` };
  const sq = await r.json();
  const ppm = pegPpm(sq, tok.decimals, fx);
  return { tok, fx, sq, ppm, problem: Math.abs(ppm) <= PEG_PPM ? null : `the mockSOL pool is ${(ppm / 1e4).toFixed(2)}% off the SOL price (run crank amm-peg --from-fx)` };
}
// Set by demo_devnet.sh, empty on a live run (2 Nov): the desk then lists every game not over.
if (process.env.SLATE_DATE !== undefined) {
  const card = await desk(`/card?date=${process.env.SLATE_DATE}`);
  const g = runSlate(card.games ?? [], { soccer: SOCCER });
  const plan = planTickets(g.length, (i) => runSide(g[i], i));
  const legs = preflightLegs(g, plan, lineLeg(g[LINE_GAME]), LAST);
  const answers = [];
  const ask = async (what, legs, stake_u) => {
    const r = await fetch(`${DESK}/quote`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ legs, stake_u, token_in: null, copied_from: null }),
    });
    answers.push({ what, status: r.status, body: await r.json().catch(() => null) });
  };
  for (const l of legs) await ask(l.what, [{ symbol: l.symbol, side: l.side }], 5_000_000);
  // 7.2a-r: each ticket whole, at the €10 preset (the payout cap, D-560).
  for (const t of preflightTickets(g, plan, lineLeg(g[LINE_GAME]), LAST)) await ask(t.what, t.legs, PREFLIGHT_TICKET_STAKE_U);
  const bad = preflightProblems(answers);
  if (SOL) {
    const peg = await solPeg();
    if (peg.problem) bad.push(`mockSOL: ${peg.problem}`);
    else say(`preflight: mockSOL pool ${(peg.ppm / 1e4).toFixed(2)}% from the SOL price (€${(peg.fx.eur_per_sol_ppm / 1e6).toFixed(2)}, ${peg.fx.sol_source}${peg.fx.sol_fallback ? ", FALLBACK price" : ""})`);
  }
  for (const b of bad) say(`FAIL preflight: ${b}`);
  if (bad.length) {
    console.log(`DEVNET E2E NOT STARTED: ${bad.length} preflight problem(s) over ${legs.length} picks${SOL ? " and the mockSOL peg" : ""}; no SOL sent`);
    process.exit(1);
  }
  say(`preflight: the desk prices all ${legs.length} picks of the run (${legs.map((l) => l.what).join(", ")}) and each ticket at €10; no SOL sent yet`);
}

// --- browser: one context per wallet ---
const browser = await chromium.launch({ executablePath: CHROME });
const pageErrors = [];
let throttled = 0;
// Every desk answer the app received, in order, and the /quote requests' stakes.
const seen = { quotes: [], quoteStakes: [], copyQuotes: [], copyAsks: [], swapQuotes: [], cashout: [], card: null, tokens: null, fx: null };
async function newPage(label) {
  const ctx = await browser.newContext({
    viewport: { width: 390, height: 844 }, deviceScaleFactor: 1, colorScheme: "dark", reducedMotion: "reduce", serviceWorkers: "block",
  });
  const p = await ctx.newPage();
  p.on("pageerror", (e) => pageErrors.push(`${label}: ${e}`));
  // The public devnet RPC throttles (HTTP 429); dioxus-solana retries those with backoff, so
  // they are counted, not failed. Any other console error fails the run.
  p.on("console", (m) => {
    if (m.type() !== "error") return;
    if (/status of 429/.test(m.text())) throttled++;
    else pageErrors.push(`${label} console: ${m.text()}`);
  });
  p.on("request", (r) => {
    if (r.url() === `${DESK}/quote` && r.method() === "POST") {
      try { seen.quoteStakes.push(JSON.parse(r.postData() ?? "{}").stake_u); } catch { /* not JSON */ }
    }
    // Every copy-quote asked, answered or not (7.2a-r: the copy page kept asking once copied).
    if (r.url() === `${DESK}/copy-quote` && r.method() === "POST") seen.copyAsks.push(Date.now());
  });
  p.on("response", async (r) => {
    const url = r.url();
    if (!url.startsWith(DESK) || r.status() !== 200) return;
    try {
      const post = r.request().method() === "POST";
      if (url.endsWith("/quote") && post) seen.quotes.push({ at: Date.now(), body: await r.json() });
      else if (url.endsWith("/copy-quote") && post) seen.copyQuotes.push({ at: Date.now(), body: await r.json() });
      else if (url.endsWith("/swap-quote") && post) seen.swapQuotes.push({ at: Date.now(), body: await r.json() });
      else if (url.includes("/cashout-quote/")) seen.cashout.push({ at: Date.now(), body: await r.json() });
      else if (url.includes("/card?")) seen.card = await r.json();
      else if (url.endsWith("/tokens")) seen.tokens = await r.json();
      else if (url.endsWith("/fx")) seen.fx = await r.json();
    } catch { /* body gone after navigation */ }
  });
  return p;
}
const page = await newPage("wallet 1");

const shots = [];
const evidenceNotes = {};
// Each screenshot is a phone-gate screen too (screen_checks.mjs). Since 31 Oct (user request)
// the app bar's pill is one euro figure (no TEST chip, no SOL, no DEPOSIT), the celebrations
// drop their "mock-filled" line and Account its honesty pill: the test-money markers are the
// faucet's "Test tokens" chip on Account and the brag post's "devnet test money".
async function shot(name, p = page) {
  const f = await screenFacts(p);
  const bad = problems(f);
  const onAccount = new URL(p.url()).pathname === "/account";
  const mock = await p.evaluate(() => {
    const vis = (sel) => [...document.querySelectorAll(sel)].some((e) => e.checkVisibility());
    const brag = document.querySelector("#celebrate-brag")?.getAttribute("href");
    const line = !brag || /test%20money/.test(brag);
    return line && !vis(".k-appbar .k-test-chip") && !vis(".k-appbar .k-balance-sol") && !vis(".k-appbar #deposit");
  });
  const pill = !onAccount || await p.locator("#account-faucet .acct-test-chip").isVisible().catch(() => false);
  check(`${name}: 390 px dark, no horizontal scroll, no trading words, values toned, tabular figures, test-money marker${onAccount ? ", the faucet's Test tokens chip" : ""}`,
    bad.length === 0 && mock && pill, `scrollWidth ${f.scrollWidth}${bad.length ? `; ${bad.join("; ")}` : ""}`);
  await p.screenshot({ path: `${OUT}/${name}.png`, fullPage: true });
  shots.push(name);
  return f.text;
}
// In-app steps and selectors: e2e_ui.mjs (shared with e2e_dry.mjs). `nav(label, p)` here keeps
// the run's argument order.
const nav = (label, p = page) => ui.nav(p, label);
const { until, drawerShown } = ui;
// The drawer's Total against a /quote answer (31 Oct: the fee table behind "x% fee included" is
// gone; the Total is the one figure it shows in the desk's own units).
const sameTotal = (shown, q) => shown.total === odds(q.odds_offered_u);

// The night's slate (slate.mjs runSlate), from the desk's /card answer to the app; frozen when
// the tickets are planned (step 2): a resolved game leaves /card, and the app's next card fetch
// would shift or shorten the list under the plan's indexes (6.2a-r run 3: "reading 'symbol'" at
// the end).
let slate = null;
const games = () => slate ?? runSlate(seen.card.games, { soccer: SOCCER });
// A winner pick on its sport's tab; a soccer pick is a long on its outcome's own symbol (D-461).
async function pick(g, which) {
  await ui.pickWinner(page, g, which);
  const p = winnerLeg(g, which);
  if (!p) throw new Error(`${g.game_id} has no ${which} pick`);
  return { g, which, team: p.label, side: p.side, symbol: p.symbol, kind: "winner" };
}
async function pickLine(g, line) {
  await ui.pickLine(page, g, line);
  return { g, which: null, team: line.label, side: line.side, symbol: line.symbol, kind: line.kind };
}

// Wallet 1 only: one ticket from the Sportsbook (and the event page for `opts.line`) → the bet drawer → Bet
// → minted. `opts.tip` turns on "Share as a tip"; `opts.bonk` pays in mockBONK ("Pay with"), `opts.sol` in
// mockSOL (the currency pill). Returns what the screens and the desk said.
async function play(name, wanted, shotNames, opts = {}) {
  await nav("Sports");
  await page.waitForSelector(".home-game .pick");
  // The previous schedina's picks left the slip when it landed.
  check(`${name}: the slip starts empty`, !(await page.locator("#slip-bar").isVisible()));
  const legs = [];
  for (const [i, which] of wanted) legs.push(await pick(games()[i], which));
  if (opts.line) {
    legs.push(await pickLine(games()[LINE_GAME], opts.line));
    if (shotNames.line) await shot(shotNames.line);
  }
  if (shotNames.card) await shot(shotNames.card);
  // The slip pill counts the picks and shows the quoted total (7.0a: "3 Picks • Total 5.94").
  const bar = await ui.openSlip(page);
  const count = ui.picksText(legs.length);
  check(`${name}: the slip pill says ${count} and the total`, new RegExp(`^${count} • Total \\d+\\.\\d\\d`).test(bar), bar);
  check(`${name} drawer: ${legs.length} legs`, (await page.locator("#slip-picks .k-pick").count()) === legs.length);
  if (opts.line) {
    const rowText = await page.locator("#slip-picks").innerText();
    check(`${name} slip: the ${opts.line.kind} leg is listed by its label`, rowText.includes(prettyLabel(opts.line.label)), prettyLabel(opts.line.label));
  }
  await ui.stakeTen(page);
  let swap = null;
  if (opts.bonk) swap = await payInBonk(name);
  else if (opts.sol) swap = await payInSol(name);
  else {
    const cta = await until("Bet • €10.00", async () => { const t = await ui.ctaText(page); return t === "Bet • €10.00" && t; }, 15).catch(() => ui.ctaText(page));
    check(`${name} drawer: the €10 preset, one purple "Bet • €10.00"`, cta === "Bet • €10.00", cta);
  }
  if (opts.tip) {
    await page.locator("#share-tip").check();
    await page.waitForSelector("#share-tip:checked");
    check(`${name}: "Share as a tip" is on`, await page.locator("#share-tip").isChecked());
  }
  // The drawer re-asks every 5 s: its Total must be one of the newest answers.
  let shownTray = null;
  const trayOk = await until("the drawer's Total = a /quote answer", async () => {
    shownTray = await drawerShown(page);
    return seen.quotes.slice(-3).some((x) => sameTotal(shownTray, x.body));
  }, 20).catch(() => false);
  check(`${name} drawer: the Total = a /quote answer`, trayOk, JSON.stringify(shownTray));
  if (shotNames.tray) await shot(shotNames.tray);

  const moves = await ui.readyToPlace(page);
  if (moves) check(`${name} drawer: a moved price blocked Bet until accepted (D-361)`, true, `${moves} accepted`);
  const before = seen.quotes.length;
  const stakesBefore = seen.quoteStakes.length;
  const swapsBefore = seen.swapQuotes.length;
  await page.locator("#gioca").click();
  // The sheet's steps tick off (1 Nov); a refusal shows in the drawer as #submit-error.
  await until(`${name} escrowed (the sheet's steps)`, async () => {
    if (await page.locator("#submit-error").count()) throw new Error(`${name}: ${await page.locator("#submit-error").innerText()}`);
    return (await page.locator("#place-steps [data-step='escrowed'].done").count()) > 0
      || new URL(page.url()).pathname === "/confirm";
  }, 120);
  if (shotNames.escrowed) await shot(shotNames.escrowed);
  await page.waitForURL("**/confirm", { timeout: 240000 });
  await page.waitForSelector("#celebration #celebrate-slip", { timeout: 30000 });
  const href = await until(`${name}: View Transaction's link`, () => ui.viewTx(page, "celebration").getAttribute("href"), 30);
  const openSig = href.match(/\/tx\/([1-9A-HJ-NP-Za-km-z]{60,90})/)[1];
  say(`${name}: open_pending ${openSig}`);
  check(`${name} celebration: View Transaction opens the open_pending on Solscan${CLUSTER === "devnet" ? " (cluster=devnet)" : ""}`,
    CLUSTER !== "devnet" || ui.solscanSig(href) === openSig, href);
  // The celebration's stake, total and payout are those of the fresh /quote the transaction signs.
  const fresh = seen.quotes.slice(before).map((x) => x.body);
  const celebTotal = await page.locator("#celebrate-total .k-stat-value").innerText();
  const celebPayout = await page.locator("#celebrate-amount .k-payout-eur").innerText();
  const signedQuote = [...fresh].reverse().find((q) => celebTotal.trim() === odds(q.odds_offered_u));
  check(`${name} celebration: Total and payout are the fresh /quote the transaction signs`,
    signedQuote && /^€[\d,]+\.\d\d$/.test(celebPayout.trim()), `${celebTotal} ${celebPayout}`);
  if (swap) {
    // Gioca re-asks the swap, then the desk at stake_u = that swap's min_out_u (L4-3).
    const sq = seen.swapQuotes.slice(swapsBefore).map((x) => x.body).pop() ?? swap.quote;
    swap.signedSwap = sq;
    check(`${name}: the signed quote's stake is the fresh swap quote's min_out_u`,
      signedQuote && signedQuote.stake_u === sq.min_out_u && seen.quoteStakes.slice(stakesBefore).includes(sq.min_out_u),
      `stake ${signedQuote?.stake_u}, min_out ${sq.min_out_u}`);
  }
  const brag = (await page.locator("#celebration #celebrate-brag").getAttribute("href")) ?? "";
  check(`${name} celebration: the brag post says devnet test money`, /devnet%20test%20money/.test(brag), brag.slice(0, 120));
  if (shotNames.minted) await shot(shotNames.minted);
  // The Ticket PDA, from the desk's /tickets by this open_pending signature.
  const t = await until(`${name}'s ticket on the desk`, async () =>
    ((await desk(`/tickets/${wallet}`)).tickets ?? []).find((x) => x.tx?.open_pending === openSig), 60);
  const onDesk = (await desk(`/ticket/${t.ticket}`)).legs ?? [];
  check(`${name}: the ticket holds the ${legs.length} picks played`,
    onDesk.length === legs.length && legs.every((l) => onDesk.some((d) => d.symbol === l.symbol && d.side === l.side)), t.ticket);
  if (opts.line) {
    check(`${name}: the ticket's ${opts.line.kind} leg is on chain as played (${opts.line.label})`,
      onDesk.some((d) => d.symbol === opts.line.symbol && d.side === opts.line.side), opts.line.symbol);
  }
  if (opts.tip) {
    check(`${name}: the desk lists it as an own tip (tipster = the wallet, not a copy)`,
      t.tipster === wallet && t.copied_from === null, `tipster ${t.tipster}`);
  }
  return { name, legs, openSig, ticket: t.ticket, quote: signedQuote, swap };
}

// In the drawer (wallet 1): BONK in the coin selector, the AMM faucet, the amount, the swap line.
async function payInBonk(name) {
  const tok = (seen.tokens?.tokens ?? []).find((t) => t.symbol === "mockBONK");
  if (!tok) throw new Error("the desk's /tokens lists no mockBONK (is MOCK_AMM_PROGRAM_ID set and the pool set up?)");
  await page.waitForSelector("#stake-currency[data-codes~='BONK']", { state: "attached", timeout: 15000 });
  await ui.payInToken(page, "BONK");
  await page.locator("#token-faucet").click();
  await page.waitForSelector("#token-note >> text=Test mockBONK received.", { timeout: 120000 });
  await until("the mockBONK balance on screen", async () => /You hold (?!0 )[\d,.]+ mockBONK/.test(await page.locator("#token-line").innerText()), 30);
  const nSwaps = seen.swapQuotes.length;
  await page.fill("#token-amount", BONK_AMOUNT);
  await page.waitForSelector("#swap-fee", { timeout: 30000 });
  await until("the /swap-quote answer", async () => seen.swapQuotes.length > nSwaps, 15);
  // The newest swap answer for this amount, and the line the screen shows for it.
  let sq = null;
  const linesOk = await until("the swap line = a /swap-quote answer", async () => {
    sq = seen.swapQuotes.slice(nSwaps).map((x) => x.body).pop();
    const shown = await drawerShown(page);
    return sq && shown.stake === usdc(sq.min_out_u) && shown.paid === `${tokenText(BONK_AMOUNT)} mockBONK`;
  }, 20).catch(() => false);
  check(`${name} drawer: you pay ${BONK_AMOUNT} mockBONK, the swap line's stake = the /swap-quote's min_out_u`, linesOk,
    sq && `fee ${sq.fee_u}, min_out ${sq.min_out_u}`);
  await until("a /quote at stake_u = min_out_u", async () => seen.quoteStakes.includes(sq.min_out_u), 20)
    .then(() => check(`${name} drawer: the desk is quoted at stake_u = min_out_u`, true, `${sq.min_out_u}`),
      () => check(`${name} drawer: the desk is quoted at stake_u = min_out_u`, false));
  check(`${name} drawer: one Bet, paying in mockBONK; tips off`,
    (await page.locator("#gioca").count()) === 1
    && /^Bet • [\d,.]+ mockBONK$/.test((await page.locator("#gioca").innerText()).trim())
    && (await page.locator("#share-tip").isDisabled()));
  await shot(`${name.toLowerCase()}-tray-mockbonk`);
  return { mint: tok.mint, decimals: tok.decimals, amountIn: sq ? BigInt(sq.amount_in) : 0n, quote: sq };
}

// Base units of a `decimals`-place mint as format::token prints them ("0.0736", "10,000").
const unitsText = (units, decimals) => {
  const s = BigInt(units).toString().padStart(decimals + 1, "0");
  return tokenText(decimals ? `${s.slice(0, -decimals)}.${s.slice(-decimals)}` : s);
};

// Wallet 1's mockSOL (E2E_SOL, V-3): 1 test mockSOL from the dev wallet page's faucet button
// (Account's faucet offers USDC and BONK only since 31 Oct), on chain before the drawer opens.
// The drawer then pays in it through the currency pill.
async function mockSolFaucet() {
  const tok = solToken(seen.tokens) ?? solToken(await desk("/tokens"));
  if (!tok) throw new Error("the desk's /tokens lists no mockSOL (is MOCK_AMM_PROGRAM_ID set and the pool set up?)");
  await ui.go(page, "/dev/wallet");
  await page.locator("#dev-mocksol:not([disabled])").click();
  await page.waitForSelector(`#test-result >> text=Test ${tok.symbol} received.`, { timeout: 120000 });
  const held = await until(`the ${tok.symbol} balance on chain`, async () => {
    const r = await rpc("getTokenAccountsByOwner", [wallet, { mint: tok.mint }, { encoding: "jsonParsed", commitment: "confirmed" }]);
    const n = Number(r.value?.[0]?.account.data.parsed.info.tokenAmount.amount ?? 0);
    return n > 0 && n;
  }, 60).catch(() => 0);
  check(`E: the dev wallet page's faucet gave test ${tok.symbol} (#dev-mocksol, the token account)`, held > 0, `${held}`);
  return tok;
}

// In the drawer (wallet 1): the stake block's currency pill to SOL (V-3: mockSOL through
// mock-amm), €10 on the presets; the app pays €10 ÷ the desk's EUR/SOL in mockSOL. The swap must
// pay that SOL's worth (the pool pegged by `crank amm-peg`), and the desk is quoted at its
// min_out_u, as for mockBONK.
async function payInSol(name) {
  const tok = solToken(seen.tokens) ?? solToken(await desk("/tokens"));
  if (!tok) throw new Error("the desk's /tokens lists no mockSOL");
  const nSwaps = seen.swapQuotes.length;
  await ui.payIn(page, "SOL");
  await ui.stakeTen(page);
  // The rate the app priced with: its last /fx answer (the desk's rate moves every 60 s).
  const fx = seen.fx?.eur_per_sol_ppm ? seen.fx : await desk("/fx");
  const amount = solFor(SOL_EUR_U, fx.eur_per_sol_ppm, tok.decimals);
  await page.waitForSelector("#swap-fee", { timeout: 30000 });
  let sq = null;
  const linesOk = await until("the mockSOL swap lines = a /swap-quote answer", async () => {
    sq = seen.swapQuotes.slice(nSwaps).map((x) => x.body).filter((q) => q.mint === tok.mint).pop();
    if (!sq) return false;
    const shown = await drawerShown(page);
    return shown.stake === usdc(sq.min_out_u) && shown.paid === `${unitsText(sq.amount_in, tok.decimals)} SOL`;
  }, 30).catch(() => false);
  check(`${name} drawer: €10 in SOL pays ${unitsText(amount, tok.decimals)} ${tok.symbol} (€10 ÷ the desk's €${(fx.eur_per_sol_ppm / 1e6).toFixed(2)} per SOL), swap lines = the /swap-quote`,
    linesOk && sq && BigInt(sq.amount_in) === amount, sq && `amount_in ${sq.amount_in}, fee ${sq.fee_u}, min_out ${sq.min_out_u}`);
  const ppm = sq ? pegPpm(sq, tok.decimals, fx) : Infinity;
  check(`${name}: the mockSOL pool pays the SOL price within 2% (crank amm-peg --from-fx; SOL ${fx.sol_source}${fx.sol_fallback ? ", fallback" : ""})`,
    Math.abs(ppm) <= PEG_PPM, `${(ppm / 1e4).toFixed(2)}% off`);
  await until("a /quote at stake_u = min_out_u", async () => sq && seen.quoteStakes.includes(sq.min_out_u), 20)
    .then(() => check(`${name} drawer: the desk is quoted at stake_u = min_out_u`, true, `${sq.min_out_u}`),
      () => check(`${name} drawer: the desk is quoted at stake_u = min_out_u`, false));
  const cta = await ui.ctaText(page);
  check(`${name} drawer: one "Bet • x SOL"`, cta === `Bet • ${unitsText(amount, tok.decimals)} SOL`, cta);
  await shot(`${name.toLowerCase()}-tray-mocksol`);
  return { mint: tok.mint, decimals: tok.decimals, amountIn: sq ? BigInt(sq.amount_in) : 0n, quote: sq, ppm };
}

// One bet in Account's bets (#bets; My bets until L8-4), by its Ticket PDA: a parlay card or a
// single's row while open, a row under Settled once closed.
const ticketCard = (t, p = page) => ui.ticketCard(p, t.ticket);
const resolved = new Set();
async function resolve(leg) {
  if (resolved.has(leg.symbol)) return; // a market resolves once (L4-1); shared games are one leg
  resolved.add(leg.symbol);
  // The pick wins: a long pick at 1, a short pick at 0.
  const price = leg.side === "long" ? "1" : "0";
  const r = await fetch(`${MOCK}/mock/resolve`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ symbol: leg.symbol, price }),
  });
  if (!r.ok) throw new Error(`/mock/resolve ${leg.symbol}: ${r.status} ${await r.text()}`);
  say(`mock: ${leg.team} wins (${leg.symbol} at ${price})`);
}

// A fresh burner on `p`, funded from the funder, with test USDC from the faucet.
async function burner(p, sol, label, names) {
  await p.goto(`${APP}/dev/wallet`);
  await p.locator("#burner").click();
  const w = (await p.locator("#wallet-address").innerText()).trim();
  say(`${label}: fresh burner wallet ${w}`);
  const out = execFileSync("solana", ["transfer", "--url", RPC, "-k", FUNDER, "--allow-unfunded-recipient",
    "--commitment", "confirmed", "--output", "json", w, sol], { encoding: "utf8" });
  const sig = out.match(/[1-9A-HJ-NP-Za-km-z]{80,90}/)[0];
  say(`${label}: funded ${sol} SOL from the funder: ${sig}`);
  await until(`${label}: the SOL balance on screen`, async () => {
    await p.locator("#sol-refresh").click();
    await p.waitForTimeout(1500); // a poll interval for the public RPC, not a wait for the page
    return !/^0\.000000000|…|unavailable/.test(await p.locator("#sol-balance").innerText());
  }, 60);
  await shot(names.fresh, p);
  // Account (31 Oct): "Your wallet" (its worth in €) and "Faucet" (coin pill, airdrop, then a
  // celebration with the transaction on the explorer).
  await nav("Account", p);
  await p.waitForSelector("#account-wallet");
  check(`${label}: Account shows the burner's address`,
    (await p.locator("#account-address").getAttribute("title").catch(() => null)) === w);
  const faucetTx = await ui.faucet(p, "USDC");
  check(`${label}: the faucet's celebration opens its transaction on Solscan`, !!ui.solscanSig(faucetTx ?? ""), faucetTx);
  const bal = await until(`${label}: 1,000 test USDC on Account`, async () => {
    const t = await ui.accountUsdc(p).catch(() => "");
    return t === "1,000.00 USDC" && t;
  }, 30);
  check(`${label}: Account's balance is 1,000.00 USDC (euros above it at the desk's rate)`, true, bal);
  await shot(names.faucet, p);
  return { wallet: w, fundSig: sig };
}
const { accountUsdc } = ui;

// Wallet 2 copies tip `B` from its copy page (/copy/<B>, 7.1e): the copy-quote for the slider's
// stake, the fee, "Copy Schedina • €…" → the celebration once minted.
async function copyTip(p, B) {
  // In-app navigation only: a page load (p.goto) would drop the in-page burner, whose key lives
  // in wasm memory, and the copy would find no wallet (5.A rehearsal, run 1). The way a
  // follower finds it (L9): Tipsters → the tipster → the open parlay → Copy Schedina.
  await nav("Tipsters", p);
  // Still connected (reached in-app): the app bar shows the balance pill, not a connect button.
  await p.waitForSelector(".k-appbar #balance, .k-appbar .wallet");
  check("copy: wallet 2 is still connected on Tipsters (app bar balance, no connect button)",
    (await p.locator(".k-appbar #balance").count()) === 1 && (await p.locator(".k-appbar .wallet").count()) === 0);
  const tipster = p.locator(`#board a.tp-row-link[href$="/tipsters/${wallet}"]`);
  await tipster.waitFor({ timeout: 60000 });
  await tipster.click();
  const row = p.locator(`#open-tips [data-ticket="${B.ticket}"] a.tp-parlay`);
  await row.waitFor({ timeout: 60000 });
  check("copy: the tipster's page lists B with its total", !!B.quote
    && (await row.locator(".tp-parlay-total").innerText()).trim() === odds(B.quote.odds_offered_u));
  await shot("c2b-tipster-profile", p);
  await row.click();
  // The parlay (/copy/<B>): its games locked to B's picks, open for copies.
  await p.waitForSelector("#parlay-head");
  check("copy: the parlay is open for copies", (await p.locator("#tip-copy-why").count()) === 0,
    (await p.locator("#tip-copy-why").count()) ? await p.locator("#tip-copy-why").innerText() : "");
  check("copy: the parlay shows B's games, each locked to B's pick",
    (await p.locator("#parlay-legs .pl-leg").count()) === B.legs.length
      && (await p.locator("#parlay-legs .pl-leg .k-outcome.selected").count()) === B.legs.length);
  // Copy Schedina: the bet drawer in copy mode, priced by /quote with copied_from.
  await p.locator("#tip-copy").click();
  await p.waitForSelector("#bet-drawer #slip-copy-fee");
  const nQuotes = seen.quotes.length;
  await ui.stakeTen(p);
  const priced = () => seen.quotes.slice(nQuotes).map((x) => x.body)
    .reverse().find((q) => q.copy_fee_u > 0 && sameLegs(q.legs, B.legs));
  const cq = await until("the drawer's copy quote (/quote with copied_from) for €10", priced, 30);
  await settle(p);
  const feeShown = (await p.locator("#slip-copy-fee").textContent()).trim();
  check("copy: the drawer's copy fee is the quote's ceil(2% of the stake)",
    cq.copy_fee_u === copyFee(cq.stake_u) && /^€\d+\.\d\d$/.test(feeShown), `stake ${cq.stake_u}, fee ${cq.copy_fee_u}, shown ${feeShown}`);
  check("copy: the drawer's CTA is \"Copy Schedina • €10.00\"", (await ui.ctaText(p)) === "Copy Schedina • €10.00", await ui.ctaText(p));
  await shot("c3-copy-quote", p);
  await ui.readyToPlace(p);
  const nCopy = seen.copyQuotes.length;
  await p.locator("#gioca").click();
  // The status rides in the CTA, then the placement celebration on /confirm (or #submit-error).
  await until("copy: escrowed or placed", async () => {
    if (await p.locator("#submit-error").count()) throw new Error(`copy: ${await p.locator("#submit-error").innerText()}`);
    return new URL(p.url()).pathname === "/confirm";
  }, 240);
  await p.waitForSelector("#celebration #celebrate-slip", { timeout: 30000 });
  const href = await until("the copy celebration's View Transaction link", () => ui.viewTx(p, "celebration").getAttribute("href"), 30);
  const openSig = href.match(/\/tx\/([1-9A-HJ-NP-Za-km-z]{60,90})/)[1];
  say(`copy: open_pending ${openSig}`);
  check(`copy celebration: View Transaction opens the open_pending on Solscan${CLUSTER === "devnet" ? " (cluster=devnet)" : ""}`,
    CLUSTER !== "devnet" || ui.solscanSig(href) === openSig, href);
  // Copy re-asked the copy-quote (flows::start_play); that fresh answer is what was signed.
  const signed = seen.copyQuotes.slice(nCopy).map((x) => x.body).pop() ?? cq;
  await shot("c3-copy-minted", p);
  const mintedAt = Date.now();
  const t = await until("the copy on the desk", async () =>
    ((await desk(`/tickets/${wallet2}`)).tickets ?? []).find((x) => x.tx?.open_pending === openSig), 60);
  check("copy: minted with copied_from = the tip, not itself a tip", t.copied_from === B.ticket && t.tipster !== wallet2,
    `copied_from ${t.copied_from}`);
  return { name: "D", openSig, ticket: t.ticket, quote: signed, legs: B.legs, mintedAt };
}
// 7.2a-r, L9: B's copiers sheet, from wallet 1 once the copy is minted, in-app: Tipsters → its
// own page → B's parlay → "Copied by 1". The sheet lists the copy (wallet 2) under "Copiers (1)";
// ✕ closes it. Nothing is sent.
async function copiersDrawer(B, D) {
  await until("the copy on B's profile card (desk /tipsters)", async () =>
    ((await desk(`/tipsters/${wallet}`)).open ?? []).some((t) => t.ticket === B.ticket && (t.copies ?? 0) >= 1), 60);
  await nav("Tipsters");
  const me = page.locator(`#board a.tp-row-link[href$="/tipsters/${wallet}"]`);
  await me.waitFor({ timeout: 60000 });
  await me.click();
  const parlay = page.locator(`#open-tips [data-ticket="${B.ticket}"] a.tp-parlay`);
  await parlay.waitFor({ timeout: 60000 });
  await parlay.click();
  await page.waitForSelector("#parlay-copiers");
  check("copiers: B's parlay says \"Copied by 1\"",
    (await page.locator("#parlay-copiers").innerText()).replace(/\s+/g, " ").trim() === "Copied by 1");
  await page.locator("#parlay-copiers").click();
  await page.waitForSelector("#copiers-sheet #copiers, #copiers-sheet #copiers-empty", { timeout: 30000 });
  const title = (await page.locator("#copiers-sheet .k-sheet-title").innerText()).trim();
  const rows = await page.locator("#copiers-sheet .k-copier").count();
  const rowText = rows ? (await page.locator("#copiers-sheet .k-copier").first().innerText()).replace(/\s+/g, " ") : "";
  check("copiers sheet: \"Copiers (1)\" on B's parlay, one row for wallet 2's copy",
    /^Copiers \(1\)/.test(title) && rows === 1 && rowText.includes(D.wallet.slice(0, 4)), `${title}; ${rows} row(s): ${rowText}`);
  await shot("c2c-copiers");
  await page.locator("#copiers-sheet .k-sheet-close").click();
  await page.waitForSelector("#copiers-sheet", { state: "detached" });
}
const sameLegs = (qlegs, legs) => qlegs.length === legs.length
  && legs.every((l) => qlegs.some((q) => q.symbol === l.symbol && q.side === l.side));

let wallet, wallet2, fundSig, fundSig2, A, B, C, D, E, offer, cashSig, cashViewSig, settleShown, page2;
try {
  // 1. Fresh wallet, SOL, test USDC.
  ({ wallet, fundSig } = await burner(page, FUND_SOL, "wallet 1", { fresh: "01-fresh-wallet", faucet: "02-faucet" }));

  // 2. The night's tickets from the Card (any slate of 3 or more games).
  await nav("Sports");
  await page.waitForSelector(".home-game .pick");
  await until("the /card answer", async () => seen.card, 10);
  slate = runSlate(seen.card.games, { soccer: SOCCER });
  const plan = planTickets(games().length, (i) => runSide(games()[i], i));
  // B also plays a spread or total on game 0 (6.0b markets, L6-2) when the card lists one; a
  // replay tape holds winners only (L6-3), so there the leg is left out with a note.
  const line = lineLeg(games()[LINE_GAME]);
  const sports = games().map((g) => (g.sport === "soccer" ? `${g.league ?? "soccer"}` : (g.league ?? "NBA")));
  say(`card: ${games().length} games (${sports.join(", ")}); A ${JSON.stringify(plan.A)} B ${JSON.stringify(plan.B)}`
    + `${line ? ` + ${line.kind} "${line.label}" on game ${LINE_GAME}` : ""}${BONK ? ` C ${JSON.stringify(plan.C)}` : ""}${SOL ? ` E ${JSON.stringify(plan.C)}` : ""}`);
  evidenceNotes.slate = games().map((g, i) => `${i}: ${g.game_id} (${g.sport ?? "basketball"})`);
  if (!line) say(`note: game ${LINE_GAME} lists no spread or total (a replay tape?): B plays winners only`);
  evidenceNotes.line = line ?? "none on the card";
  A = await play("A", plan.A, { card: "03-card", tray: "04-tray", escrowed: "05-escrowed", minted: "06-minted" });
  B = await play("B", plan.B, { ...(TIP ? { tray: "04b-tray-tip" } : {}), ...(line ? { line: "04c-event-line" } : {}) }, { tip: TIP, line });

  // 3. L5: a second wallet copies the tip, before any of its games resolves.
  if (COPY) {
    page2 = await newPage("wallet 2");
    ({ wallet: wallet2, fundSig: fundSig2 } = await burner(page2, FUND2_SOL, "wallet 2", { fresh: "c1-fresh-wallet", faucet: "c2-faucet" }));
    D = await copyTip(page2, B);
    await copiersDrawer(B, { wallet: wallet2 });
  }
  // 4. L5: ticket C paid in mockBONK; L7: ticket E paid in mockSOL (the pool pegged first).
  if (BONK) C = await play("C", plan.C, { minted: "06c-minted-mockbonk" }, { bonk: true });
  if (SOL) {
    await mockSolFaucet();
    E = await play("E", plan.C, { minted: "06e-minted-mocksol" }, { sol: true });
  }

  // 5. Leg 1 of A resolves; the desk resolves it on chain and rolls; Cash out in one tap (1 Nov:
  // the desk's signed offer is signed at once; a lower offer than shown would stop for a confirm).
  await resolve(A.legs[0]);
  await ui.bets(page);
  const a = ticketCard(A);
  await a.waitFor({ timeout: 60000 });
  await until("A leg 1 won on screen", async () => (await a.locator(".mb-leg").first().getAttribute("data-status")) === "Won", 120);
  const ask = a.locator("button.mb-cashout", { hasText: /^Cash out €/ });
  await ask.waitFor({ timeout: 180000 });
  await shot("07-leg1-won");
  const nOffers = seen.cashout.length;
  const shownU = Number(await a.locator(".mb-cash-usdc").innerText().then((t) => Math.round(Number(t.replace(/[^\d.]/g, "")) * 1e6)));
  const heroEur = (await a.locator(".mb-cash-eur").innerText()).trim();
  await shot("08-offer");
  await ask.click();
  await until("the /cashout-quote answer", async () => seen.cashout.length > nOffers, 30);
  offer = seen.cashout[seen.cashout.length - 1].body;
  // Only a lower offer than the figure tapped stops for a confirm.
  if (await a.locator("button.mb-cashout-confirm").count()) {
    check("A offer: a lower offer waits for a confirm", offer.c_u * 100 < shownU * 99, `${usdc(offer.c_u)} vs ${shownU}`);
    await a.locator("button.mb-cashout-confirm").click();
  }

  // The cash-out celebration (7.1f; 31 Oct: the payout and the two buttons only): the figure is
  // the signed offer's; the chain side (cash_out paid exactly C) is checked from the evidence.
  await page.waitForSelector("#cashout-celebration #celebrate-amount", { timeout: 120000 });
  const paidShown = (await page.locator("#cashout-frame .k-payout-eur").innerText()).trim();
  // The figure is the signed offer's in euros (within a cent or so of the live figure tapped).
  check("A celebration: Total Payout Received is a euro amount", /^€[\d,]+\.\d\d$/.test(paidShown), `${paidShown} (tapped ${heroEur}, offer ${usdc(offer.c_u)})`);
  // View Transaction (guidelines §8): the cash_out on Solscan, the same signature Account's bets link.
  const cashHref = await until("the cash-out celebration's View Transaction link", () => ui.viewTx(page, "cashout-celebration").getAttribute("href"), 60).catch(() => null);
  cashViewSig = cashHref?.match(/\/tx\/([1-9A-HJ-NP-Za-km-z]{60,90})/)?.[1] ?? null;
  check(`A celebration: View Transaction is a Solscan link${CLUSTER === "devnet" ? " (cluster=devnet)" : ""}`,
    !!cashViewSig && (CLUSTER !== "devnet" || ui.solscanSig(cashHref) === cashViewSig), cashHref ?? "no link");
  await shot("08b-cashout-celebration");
  await page.locator("#cashout-close").click();
  await page.waitForSelector("#cashout-celebration", { state: "detached" });
  // Once cashed out, Account's bets show it under Settled, where the row links its cash_out
  // transaction (the desk's tx.cash_out).
  await a.locator(".closing", { hasText: "Cashed out" }).waitFor({ timeout: 120000 });
  cashSig = (await a.locator(".closing a").getAttribute("href")).match(/\/tx\/(\w+)/)[1];
  say(`A: cash_out ${cashSig}`);
  check("A: the celebration's View Transaction is the cash_out Account's bets link", cashViewSig === cashSig, `${cashViewSig} / ${cashSig}`);
  await until("A cashed out with the paid amount", async () =>
    (await a.locator(".mb-result").innerText()).trim() === "Cashed out"
      && Number(await a.locator(".mb-paid").getAttribute("data-paid-u")) === offer.c_u, 60);
  await shot("09-cashed-out");

  // 6. B's legs resolve for its picks; the crank settles B (and its copy).
  for (const leg of B.legs) await resolve(leg);
  await page.locator("#my-bets-tabs button", { hasText: "Settled" }).click();
  await until("B settled (Won, paid) on screen", async () => {
    const card = ticketCard(B);
    return (await card.count()) === 1 && (await card.locator(".mb-result").innerText()).trim() === "Won"
      && (await card.locator(".closing", { hasText: "Settled" }).count()) === 1;
  }, 240);
  settleShown = Number(await ticketCard(B).locator(".mb-paid").getAttribute("data-paid-u"));
  // The wallet line's balance is the chain's (re-read when a ticket closes).
  const onChain = async (w) => {
    const r = await rpc("getTokenAccountsByOwner", [w, { mint: USDC_MINT }, { encoding: "jsonParsed", commitment: "confirmed" }]);
    return Number(r.value[0].account.data.parsed.info.tokenAmount.amount);
  };
  let balShown = "";
  await nav("Account");
  const balOk = await until("Account shows the on-chain balance", async () => {
    balShown = await accountUsdc(page);
    return balShown === usdc(await onChain(wallet));
  }, 30).catch(() => false);
  check("Account's test USDC = the token account on chain", balOk, balShown);
  await shot("10-settled");
  if (D) {
    // Wallet 2 sat on the copy's celebration while B's games resolved: the page asks no price.
    const late = seen.copyAsks.filter((at) => at > D.mintedAt).length;
    check("copy page: no copy-quote asked once the copy was minted", late === 0, `${late} asked`);
    await ui.bets(page2);
    await page2.locator("#my-bets-tabs button", { hasText: "Settled" }).click();
    await until("the copy settled (Won) on wallet 2's screen", async () => {
      const card = ticketCard(D, page2);
      return (await card.count()) === 1 && (await card.locator(".mb-result").innerText()).trim() === "Won"
        && (await card.locator(".closing", { hasText: "Settled" }).count()) === 1;
    }, 240);
    await shot("c4-copy-settled", page2);
  }
  // L5 evidence (5.A): the settled tip's wallet on the Tipsters list (L9: everyone with a tip),
  // then House. Wallet 1 only; nothing is sent.
  if (TIP) {
    await nav("Tipsters");
    const onBoard = await until("wallet 1 on the Tipsters list", async () =>
      (await page.locator(`#board a.tp-row-link[href$="${wallet}"]`).count()) === 1, 60).catch(() => false);
    check("Tipsters: the settled tip's wallet is on the list", onBoard);
    await shot("11-tipsters");
  }
  // House (`/house`, off Account since 31 Oct); in-app, so the in-tab burner survives (D-230).
  await ui.go(page, "/house");
  await page.waitForSelector("#account-house #house-hedger", { timeout: 30000 });
  await shot("12-house");

  // 7. End of the night: every other game of the run resolves (each once, for its pick), so every
  // ticket on these markets (A is already cashed out; an earlier aborted run's ticket on the same
  // picks) settles and the vault locks nothing. Waits until the program holds no open ticket.
  const all = [...A.legs, ...B.legs, ...(C?.legs ?? []), ...(E?.legs ?? [])];
  for (const leg of endOfNightLegs(slate, resolutionOrder(plan.A, plan.B, LAST ? plan.C : []), all)) {
    await resolve(leg);
  }
  let closed = false;
  for (let i = 0; i < 18 && !closed; i++) {
    await sleep(10000); // `crank status` reads every program account: spaced for the public RPC
    const st = execFileSync(env("CRANK_BIN"), ["--rpc", RPC, "status"], { encoding: "utf8" });
    closed = /locked 0\.000000/.test(st) && /open 0,/.test(st);
  }
  check("end of night: no open ticket, vault locked 0 (crank status)", closed);
} catch (e) {
  check("browser run", false, String(e).replaceAll(RPC, "<rpc>").slice(0, 400));
  await page.screenshot({ path: `${OUT}/failure.png`, fullPage: true }).catch(() => {});
  if (page2) await page2.screenshot({ path: `${OUT}/failure-wallet2.png`, fullPage: true }).catch(() => {});
}
check("no page errors", pageErrors.length === 0, pageErrors.slice(0, 3).join(" | "));
say(`browser: ${throttled} RPC answers throttled (429) and retried`);
await browser.close();

// --- evidence: the desk's tickets, every devnet signature, the token balance changes ---
const evidence = {
  wallet, wallet2: wallet2 ?? null, fund_sol: FUND_SOL, fund2_sol: COPY ? FUND2_SOL : null, fund_sig: fundSig,
  fund2_sig: fundSig2 ?? null, flags: { tip: TIP, copy: COPY, bonk: BONK, sol: SOL, soccer: SOCCER }, rpc_429_in_browser: throttled,
  tickets: {}, txs: [], checks,
};
if (wallet) {
  const mine = (await desk(`/tickets/${wallet}`)).tickets ?? [];
  const theirs = wallet2 ? (await desk(`/tickets/${wallet2}`)).tickets ?? [] : [];
  const byOpen = (sig) => [...mine, ...theirs].find((t) => t.tx?.open_pending === sig);
  const tA = A && byOpen(A.openSig);
  const tB = B && byOpen(B.openSig);
  const tC = C && byOpen(C.openSig);
  const tD = D && byOpen(D.openSig);
  const tE = E && byOpen(E.openSig);
  const named = [["A", tA], ["B", tB], ["C", tC], ["D", tD], ["E", tE]].filter(([, t]) => t);
  const bonkMint = C?.swap?.mint;
  const solMint = E?.swap?.mint;
  const addrs = [wallet, wallet2, ...named.map(([, t]) => t.ticket)].filter(Boolean);
  const sigs = new Map();
  for (const addr of addrs) {
    for (const s of await rpc("getSignaturesForAddress", [addr, { limit: 100, commitment: "confirmed" }])) sigs.set(s.signature, s);
  }
  for (const [sig, s] of sigs) {
    let tx = null;
    for (let i = 0; i < 10 && !tx; i++) {
      tx = await rpc("getTransaction", [sig, { encoding: "json", commitment: "confirmed", maxSupportedTransactionVersion: 0 }]);
      if (!tx) await sleep(2000);
    }
    if (!tx) { evidence.txs.push({ sig, slot: s.slot, ix: "?", err: "not returned" }); continue; }
    // A v0 transaction's lookup-table accounts are in meta.loadedAddresses.
    const keys = [...tx.transaction.message.accountKeys, ...(tx.meta.loadedAddresses?.writable ?? []),
      ...(tx.meta.loadedAddresses?.readonly ?? [])];
    const logs = tx.meta.logMessages ?? [];
    const ixs = [...new Set(logs.map((l) => l.match(/^Program log: Instruction: (\w+)/)?.[1]).filter(Boolean))];
    const delta = (owner, mint) => {
      if (!owner || !mint) return 0;
      const bal = (arr) => (arr ?? []).find((b) => b.owner === owner && b.mint === mint)?.uiTokenAmount.amount;
      return Number(BigInt(bal(tx.meta.postTokenBalances) ?? 0) - BigInt(bal(tx.meta.preTokenBalances) ?? 0));
    };
    const sysTransfer = keys.includes("11111111111111111111111111111111") && !keys.includes(PROGRAM);
    evidence.txs.push({
      sig, slot: tx.slot, block_time: tx.blockTime, err: tx.meta.err, signer: keys[0],
      ix: ixs.join("+") || (sysTransfer ? "SystemTransfer" : "?"),
      user_usdc_delta_u: delta(wallet, USDC_MINT),
      copier_usdc_delta_u: delta(wallet2, USDC_MINT),
      user_bonk_delta: delta(wallet, bonkMint),
      user_sol_delta: delta(wallet, solMint),
      // Latest ticket first: the copy's finalize also lists its original (L2-4), and is the copy's.
      ticket: [...named].reverse().find(([, t]) => keys.includes(t.ticket) || t.tx?.open_pending === sig)?.[0] ?? null,
    });
  }
  evidence.txs.sort((x, y) => x.slot - y.slot);
  for (const [k, t] of named) evidence.tickets[k] = t;
  // A second desk holding the crank key may race this one's settle; the program refuses the
  // duplicate (report.mjs). Those are listed, not failed, and never count as a ticket's own.
  for (const x of evidence.txs) if (refusedDuplicate(x, evidence.txs, CRANK)) x.refused_duplicate = true;
  const dups = evidence.txs.filter((x) => x.refused_duplicate);
  for (const x of dups) say(`note: a duplicate ${x.ix} of ${x.ticket} by the crank key was refused (another desk won the race): ${x.sig}`);
  const txOf = (k, name) => evidence.txs.filter((x) => x.ticket === k && x.err === null && x.ix.toLowerCase().includes(name));
  const opened = (sig) => evidence.txs.find((x) => x.sig === sig);

  check("every transaction of the run succeeded", evidence.txs.length > 0 && evidence.txs.every((x) => x.err === null || x.refused_duplicate),
    `${evidence.txs.length} transactions${dups.length ? `, ${dups.length} refused duplicate finalize/settle(s) from another desk with the crank key` : ""}`);
  if (tA && A.quote) {
    check("A: open_pending moved −stake from the wallet", opened(A.openSig)?.user_usdc_delta_u === -A.quote.stake_u, `${opened(A.openSig)?.user_usdc_delta_u}`);
    check("A: the ticket's payout and odds are the signed quote's", tA.payout_u === A.quote.payout_u && tA.odds_offered_u === A.quote.odds_offered_u);
    check("A: finalize and resolve_leg sent by the crank", txOf("A", "finalize").every((x) => x.signer === CRANK) && txOf("A", "finalize").length === 1 && txOf("A", "resolveleg").length >= 1 && txOf("A", "resolveleg").every((x) => x.signer === CRANK));
    const cash = evidence.txs.find((x) => x.sig === cashSig);
    check("A: cash_out paid the wallet exactly C of the signed offer, and the desk says paid C", offer && cash?.user_usdc_delta_u === offer.c_u && tA.paid_u === offer.c_u && tA.tx?.cash_out === cashSig,
      `C ${offer?.c_u}, delta ${cash?.user_usdc_delta_u}, paid ${tA.paid_u}`);
    // Mode M: the desk's fills say venue mock (Account's pill says so on screen).
    const fills = tA.fills ?? [];
    const feeU = fills.reduce((s, f) => s + f.builder_fee_u, 0);
    check("A: the desk's fills are mock fills (venue mock)",
      fills.length > 0 && fills.every((f) => f.venue === "mock"),
      `${fills.length} fills, builder fee ${usdcFine(feeU)}`);
  }
  if (tB && B.quote) {
    check("B: open_pending moved −stake from the wallet", opened(B.openSig)?.user_usdc_delta_u === -B.quote.stake_u);
    const settle = txOf("B", "settle");
    check("B: settle (by the crank) paid the wallet the full payout P shown at Gioca", settle.length === 1 && settle[0].signer === CRANK &&
      settle[0].user_usdc_delta_u === B.quote.payout_u && tB.paid_u === B.quote.payout_u && tB.state === "settled" &&
      settleShown === B.quote.payout_u,
      `P ${B.quote.payout_u}, delta ${settle[0]?.user_usdc_delta_u}, paid ${tB.paid_u}, shown ${settleShown}`);
    if (TIP) check("B: an own tip on the desk (tipster = the wallet)", tB.tipster === wallet && tB.copied_from === null);
  }
  if (COPY) {
    check("copy: minted", !!tD, D?.ticket ?? "no copy ticket");
    if (tD && D.quote) {
      const stake = D.quote.stake_u;
      const fee = D.quote.copy_fee_u;
      check("copy: open_pending escrowed stake + the 2% copy fee from wallet 2",
        opened(D.openSig)?.copier_usdc_delta_u === -(stake + fee) && fee === copyFee(stake),
        `delta ${opened(D.openSig)?.copier_usdc_delta_u}, stake ${stake}, fee ${fee}`);
      // finalize pays the tipster floor(floor(fee × r) × share / 1e4): at most 80% of the fee.
      const fin = txOf("D", "finalize");
      const paid = fin[0]?.user_usdc_delta_u ?? 0;
      const board = await desk(`/tipsters/${wallet}`).catch(() => ({}));
      check("copy: its finalize (by the crank) paid the tipster's USDC account its share of the fee, as the desk records",
        fin.length === 1 && fin[0].signer === CRANK && paid > 0 && paid <= Math.floor(fee * TIPSTER_SHARE_BPS / 10_000)
          && board?.tipster?.fees_earned_u === paid,
        `tipster +${paid}, fee ${fee}, desk fees_earned_u ${board?.tipster?.fees_earned_u}`);
      const settle = txOf("D", "settle");
      check("copy: settled with the tip, paying wallet 2 the copy's payout",
        tD.state === "settled" && settle.length === 1 && settle[0].copier_usdc_delta_u === tD.payout_u && tD.paid_u === tD.payout_u,
        `payout ${tD.payout_u}, delta ${settle[0]?.copier_usdc_delta_u}`);
    }
  }
  if (BONK) {
    check("C: minted", !!tC, C?.ticket ?? "no mockBONK ticket");
    if (tC && C.swap?.signedSwap) {
      const sq = C.swap.signedSwap;
      const o = opened(C.openSig);
      check("C: one transaction swapped mockBONK and opened the schedina", /Swap/.test(o?.ix ?? "") && /OpenPending/.test(o?.ix ?? ""), o?.ix);
      check("C: the swap sold exactly the mockBONK asked", BigInt(-(o?.user_bonk_delta ?? 0)) === BigInt(sq.amount_in), `${o?.user_bonk_delta}`);
      check("C: stake on chain = the swap quote's min_out_u; the wallet kept the swap's surplus (≥ 0 USDC)",
        tC.stake_u === sq.min_out_u && (o?.user_usdc_delta_u ?? -1) >= 0,
        `stake ${tC.stake_u}, min_out ${sq.min_out_u}, wallet USDC ${o?.user_usdc_delta_u}`);
    }
  }
  if (SOL) {
    check("E: minted", !!tE, E?.ticket ?? "no mockSOL ticket");
    if (tE && E.swap?.signedSwap) {
      const sq = E.swap.signedSwap;
      const o = opened(E.openSig);
      check("E: one transaction swapped mockSOL and opened the schedina", /Swap/.test(o?.ix ?? "") && /OpenPending/.test(o?.ix ?? ""), o?.ix);
      check("E: the swap sold exactly the mockSOL the app asked (€10 at the SOL price)", BigInt(-(o?.user_sol_delta ?? 0)) === BigInt(sq.amount_in), `${o?.user_sol_delta}`);
      check("E: stake on chain = the swap quote's min_out_u; the wallet kept the swap's surplus (≥ 0 USDC)",
        tE.stake_u === sq.min_out_u && (o?.user_usdc_delta_u ?? -1) >= 0,
        `stake ${tE.stake_u}, min_out ${sq.min_out_u}, wallet USDC ${o?.user_usdc_delta_u}, pool ${(E.swap.ppm / 1e4).toFixed(2)}% off the SOL price`);
      check("E: settled at the end of the night", tE.state === "settled" || (await desk(`/ticket/${tE.ticket}`).catch(() => ({}))).state === "settled", tE.state);
    }
  }
  const faucet = evidence.txs.filter((x) => /faucetusdc/i.test(x.ix));
  check("faucet: +1,000 test USDC", faucet.filter((x) => x.user_usdc_delta_u === 1_000_000_000).length === 1
    && (!COPY || faucet.filter((x) => x.copier_usdc_delta_u === 1_000_000_000).length === 1));
}
evidence.screenshots = shots;
evidence.notes = evidenceNotes;
writeFileSync(`${OUT}/evidence.json`, JSON.stringify(evidence, null, 1));
// Solscan, as the app links them (V-9).
const ex = (s) => CLUSTER === "devnet" ? `[${s.slice(0, 8)}…](https://solscan.io/tx/${s}?cluster=devnet)` : `\`${s.slice(0, 8)}…\``;
const usd = (u) => (u ? (u / 1e6).toFixed(6) : "0");
const md = [
  "| # | Instruction | Ticket | Signer | Wallet USDC Δ | Wallet 2 USDC Δ | Signature |",
  "| --- | --- | --- | --- | --- | --- | --- |",
  ...evidence.txs.map((x, i) => `| ${i + 1} | ${x.ix} | ${x.ticket ?? "-"} | \`${x.signer?.slice(0, 4)}…\` | ${usd(x.user_usdc_delta_u)} | ${usd(x.copier_usdc_delta_u)} | ${ex(x.sig)} |`),
  "",
  ...checks.map((c) => `- ${c.ok ? "ok" : "**FAIL**"}: ${c.what}${c.detail ? ` (${c.detail})` : ""}`),
].join("\n");
writeFileSync(`${OUT}/evidence.md`, md + "\n");
const failed = checks.filter((c) => !c.ok);
console.log(failed.length ? `DEVNET E2E FAILED: ${failed.length} of ${checks.length} checks` :
  `DEVNET E2E PASS: ${checks.length} checks, ${evidence.txs.length} devnet transactions, wallet ${wallet}${wallet2 ? `, wallet 2 ${wallet2}` : ""}`);
process.exit(failed.length ? 1 : 0);
