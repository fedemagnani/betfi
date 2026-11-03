// Phone gate (2.E, 4.B, rewritten for the L6-1 screens by 6.2a-g). Every screen of the BetFi app
// at 390 px, the top-level screens and the desktop composition at 1024 and 1440 px. On each one
// (scripts/screen_checks.mjs): dark whatever the system prefers, no horizontal scroll, no trading
// words, every market toned (guidelines §2: lowest green, highest purple), every figure in
// tabular Plus Jakarta Sans, plus the screen's own checks below. Chrome (7.0a, V-7): the app bar
// (one euro figure, user request 31 Oct) and the bottom nav on top-level screens, a back header on sub-pages, one
// 480 px column on desktop. No screen shows the honesty pill (31 Oct: it left Account; the
// faucet's "Test tokens" chip and the brag posts say test money).
// Screenshots go to <out>; the JSON report goes to stdout, then a one-line summary as the last
// line. Exits 1 on any failed check.
//
//   PLAYWRIGHT_CORE=…/playwright-core/index.mjs node scripts/phone_gate.mjs <app-url> <out-dir>
//
// Needs playwright-core (PLAYWRIGHT_CORE, e.g. the repo root's node_modules/playwright-core/
// index.mjs) and a headless Chromium (CHROME_PATH). `just phone-gate` builds the
// demo build, serves it, runs this and stops the server.
//
// GATE_MODE=demo (default), a demo build (no desk, example data):
//   390 px  Sportsbook (sport tabs, live and upcoming cards, 1 / X / 2, event links) → three picks (scripts/slate.mjs, any slate of 3
//           or more) → the slip pill's "3 Picks • Total X" → the bet drawer over the Sportsbook
//           (7.1b: three toned picks, "3 PICKS", the stake block with the EUR pill, slider and
//           €10 €25 €50 €100, Total and Payout, one "Bet • €10.00", the fee line), What others
//           think, the fee details (the table = the example quote), the slider's minimum, paying in
//           mockBONK (swap fee line, stake = min_out, tips off) and back, Share as a tip, the
//           price-moved rule (a banner blocks Bet) → Bet: escrowed / hedging in the CTA → the
//           placement celebration on /confirm (tx badge, payout, slip card, View Transaction and
//           Brag on 𝕏 only) → Account's bets (#bets, L8-4: open: card, legs with status,
//           cash-out; the signed offer and the signing step; settled) → receipts (plain, settled tip, open tip's copy
//           link) → Tipsters (board, orders, everyone) → a tipster's page → Copy confirm (/copy/<id>,
//           7.1e: the 2% copy fee, lock, legs, stake block, summary; €15 on the slider; progress on
//           the button, then the celebration on the page) → Event Detail (7.1c: live card, outcome
//           cards toned, Staked Sentiment, a pick and its swap, each More markets kind the card
//           lists, the CTA opening the slip) → Account (wallet, balances, DEPOSIT, profile, the
//           house; /house redirects there, /live is gone: V-7, 7.1f), the dev wallet. Then Home
//           once more with the system set to
//           light (still dark, D-310).
//   1024 and 1440 px  the centred 480 px column (V-7) on Home (app bar: wordmark, balance pill
//           (euros only), account; ticker; bottom nav; no side columns), three
//           picks and the slip pill, then every top-level screen and the event, tipster, copy
//           and receipt pages.
//
// GATE_MODE=live (3.C: scripts/demo_devnet.sh --e2e runs it against its running desk): no wallet,
// nothing is signed. Home, picks, the slip with the desk's quote (and its fee table), the minimum
// stake, paying in the desk's token when it lists one, Account's bets asking to connect, the event page
// with its spread and total tabs when the desk's card lists them pre-game, Account, the dev
// wallet; the desktop pass on the top-level screens and the event page.
//
// DESIGN_URL=<url of a design build> (optional, both modes): the drawer on the mockup's seeded
// slip (D-360) and the placement celebration on the mockup's ticket, at 390 and 1440 px; at 390 px
// the event page on the design card (More markets) and its soccer 1-X-2 game (7.1c).
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { problems, screenFacts, settle } from "./screen_checks.mjs";
import { byStart, desktopReady, gatePicks } from "./slate.mjs";
import { bets, coinCodes, nav as navTo, pickCoin, stakeTen } from "./e2e_ui.mjs";

const { chromium } = await import(process.env.PLAYWRIGHT_CORE ?? "playwright-core");
const [base, out] = [process.argv[2], process.argv[3]];
if (!base || !out) {
  console.error("usage: node scripts/phone_gate.mjs <app-url> <out-dir>");
  process.exit(2);
}
const LIVE = (process.env.GATE_MODE ?? "demo") === "live";
const DESIGN_URL = process.env.DESIGN_URL || null;
const CHROME = process.env.CHROME_PATH
  ?? `${homedir()}/Library/Caches/ms-playwright/chromium_headless_shell-1208/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const fixture = (f) => JSON.parse(readFileSync(new URL(`../fixtures/desk/${f}`, import.meta.url), "utf8"));
const browser = await chromium.launch({ executablePath: CHROME });
const report = [];
let failed = false;
const fail = (pass, name, why) => { failed = true; report.push({ scheme: "dark", pass, name, ok: false, why }); };

// The app's formatting (app/src/format.rs) for the demo fixtures' integers.
const group = (n) => n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
const cents = (u) => `${group(Math.floor(u / 1e6))}.${String(Math.floor((u % 1e6) / 1e4)).padStart(2, "0")}`;
const usdc = (u) => `${cents(u)} USDC`;
const oddsText = (u) => cents(u);

// One browser context at `width`; `check(name, extra)` measures the page, screenshots it and
// records one report row. `extra` holds the screen's own checks (name → boolean).
// Reduced motion by default: screenshots and figures are read at rest (the celebrations' count-up
// and confetti, 31 Oct); `celebrationMotion` checks them with motion on.
async function session(pass, width, { height = 844, scale = 2, colorScheme = "dark", reducedMotion = "reduce" } = {}) {
  const ctx = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: scale, colorScheme, reducedMotion, serviceWorkers: "block" });
  await ctx.route(/twitch\.tv/i, (r) => r.abort());
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  const twitch = [];
  page.on("request", (r) => { if (/twitch\.tv/i.test(r.url())) twitch.push(r.url()); });
  // The desk's /card answer (live mode), for the event page's tabs.
  let card = null;
  page.on("response", async (r) => {
    if (/\/card\?/.test(r.url()) && r.status() === 200) card = await r.json().catch(() => card);
  });
  const rows = [];
  const check = async (name, extra = {}) => {
    const f = await screenFacts(page);
    const bad = problems(f);
    // The honesty pills a screen shows (none since 31 Oct).
    const pills = f.pills.map((p) => p.id);
    const extraBad = Object.entries(extra).filter(([, v]) => !v).map(([k]) => k);
    const ok = bad.length === 0 && extraBad.length === 0;
    if (!ok) failed = true;
    const row = {
      scheme: colorScheme, pass, name, width, scrollWidth: f.scrollWidth, pairs: f.pairs, figures: f.figures, pills,
      ...(bad.length ? { problems: bad } : {}), ...(extraBad.length ? { failedChecks: extraBad } : {}), ok,
    };
    rows.push(row);
    report.push(row);
    await page.screenshot({ path: `${out}/${pass}-${name}.png`, fullPage: width < 1000 });
    return f;
  };
  const has = async (sel) => (await page.locator(sel).count()) > 0;
  const shown = (sel) => page.locator(sel).first().isVisible().catch(() => false);
  const close = async () => {
    if (errors.length) { failed = true; report.push({ scheme: colorScheme, pass, errors }); }
    if (twitch.length && width < 600) fail(pass, "no-twitch", twitch);
    await ctx.close();
  };
  return { pass, ctx, page, check, has, shown, rows, close, card: () => card };
}

// Runs one pass; an exception (a selector that never came) is a failed check with a screenshot,
// and the context is closed either way.
async function guarded(s, fn) {
  try {
    await fn();
  } catch (e) {
    fail(s.pass, "exception", String(e).split("\n")[0].slice(0, 300));
    await s.page.screenshot({ path: `${out}/${s.pass}-failure.png`, fullPage: true }).catch(() => {});
  } finally {
    await s.close();
  }
}

// Client-side navigation (the router's own history), so the slip and any in-tab wallet survive.
const go = (page, path) => page.evaluate((p) => {
  history.pushState(null, "", p);
  dispatchEvent(new PopStateEvent("popstate"));
}, path);

// Empties the slip through the drawer's CLEAR (picks on spread and total lines included).
async function clearSlip(page) {
  if (!(await page.locator("#bet-drawer").isVisible().catch(() => false))) {
    if (!(await page.locator("#slip-bar").isVisible())) return;
    await page.locator("#slip-bar").click();
    await page.waitForSelector("#bet-drawer");
  }
  await page.locator("#sheet-clear").click();
  await page.waitForSelector("#bet-drawer", { state: "detached" });
}

// The Sportsbook's games as shown (7.1a): each card's away and home pick, true where enabled.
// By class, not position: a card lists "1" (home) first, then "X" on soccer, then "2" (away).
const homeRows = (page) => page.locator(".home-game").evaluateAll((games) => games.map((g) => {
  const on = (c) => { const b = g.querySelector(`.pick-${c}`); return !!b && !b.disabled; };
  return { away: on("away"), home: on("home") };
}));

// The Sportsbook's own checks (7.1a, BetFi C2 `betfi_sportsbook_equal_odds`): the sport tabs
// (only sports the card holds, one pressed), a live card per started game (LIVE, "League • 67′",
// the live score per team ("–" until the desk has one), "View Market Volume", the first one `#home-hero`), "UPCOMING MATCHES", each
// card's two teams side by side (31 Oct: side 1 left, side 2 right, no market title), outcome
// buttons "1 2" or "1 X 2", every card's header a link to its event, no L6 league chips, never
// the word "odds" (guidelines §2).
async function sportsbookChecks(page) {
  // Team logos (31 Oct): give them a moment to arrive before judging them.
  await page.waitForFunction(() => [...document.querySelectorAll("#home .k-match-logo img")].every((i) => i.complete), null, { timeout: 5000 }).catch(() => {});
  return page.evaluate(() => {
    const all = (sel, root = document) => [...root.querySelectorAll(sel)];
    const games = all("#home .home-game");
    const live = all("#home .home-live");
    const upcoming = all("#home .home-row");
    const tabs = all("#sport-tabs .k-sport-tab");
    const labels = (g) => all(".k-outcome-label", g).map((e) => e.textContent.trim()).join(" ");
    return {
      tabs: tabs.length >= 1 && tabs.filter((t) => t.getAttribute("aria-pressed") === "true").length === 1
        // The label itself (the chip's optional pick counter, 31 Oct, is a separate badge).
        && tabs.every((t) => /^(Basketball|Soccer)$/.test(t.querySelector(":scope > span:not(.k-sport-count)")?.innerText.trim() ?? ""))
        && tabs.every((t) => { const c = t.querySelector(".k-sport-count"); return !c || /^[1-9]\d*$/.test(c.innerText.trim()); }),
      noLeagueChips: !document.querySelector(".home-chips, #home .k-chip"),
      cards: games.length >= 1 && games.length === live.length + upcoming.length,
      liveCards: live.every((g) => /LIVE/.test(g.querySelector(".k-badge.live")?.innerText ?? "")
        && / • (\d+(\+\d+)?′|Half-time|Q\d|OT|\S+)/.test(g.querySelector(".k-match-league")?.innerText ?? "")
        && all(".k-match-value", g).length === 2 && all(".k-match-value", g).every((v) => /^(\d+|–)$/.test(v.innerText.trim()))
        && /Market volume/.test(g.querySelector(".k-action")?.innerText ?? ""))
        && (live.length === 0 || !!document.querySelector("#home-hero.k-match.live")),
      upcomingTitle: upcoming.length === 0 || /^Upcoming$/i.test(document.querySelector("#home-upcoming-title")?.innerText.trim() ?? ""),
      noMarketTitle: !document.querySelector("#home .k-match-right"),
      sideBySide: games.every((g) => {
        const [a, b] = [g.querySelector(".k-match-team.first"), g.querySelector(".k-match-team.second")].map((e) => e?.getBoundingClientRect());
        return a && b && a.width > 0 && b.width > 0 && a.right <= b.left && Math.abs(a.top + a.height / 2 - (b.top + b.height / 2)) < 2;
      }),
      outcomes: games.every((g) => ["1 2", "1 X 2"].includes(labels(g))),
      eventLinks: games.every((g) => g.querySelector(`a.k-match-head[href="/event/${g.dataset.gameId}"]`)),
      noOddsWord: !/\bodds\b/i.test(document.querySelector("#home")?.innerText ?? ""),
      // Every team logo shown actually loads (no broken image); a team with none keeps its code
      // circle on the live hero.
      logosLoad: all("#home .k-match-logo img").every((i) => i.complete && i.naturalWidth > 0),
      liveTeamMark: live.every((g) => all(".k-match-team", g).every((t) => t.querySelector(".k-match-logo, .k-match-code"))),
    };
  });
}

// Shows the Sportsbook tab of `game`'s sport (a card lists one sport at a time, V-2).
async function showSport(page, game) {
  const sport = game?.sport === "soccer" ? "soccer" : "basketball";
  const tab = page.locator(`#sport-${sport}`);
  if ((await tab.count()) && (await tab.getAttribute("aria-pressed")) !== "true") await tab.click();
}

// Opens `game`'s Event Detail in-app from the Sportsbook (its sport's tab, the card's header link).
async function openEvent(page, nav, game) {
  await nav("Sports");
  await page.waitForSelector(".home-game .pick");
  await showSport(page, game);
  await page.locator(`.home-game[data-game-id="${game.game_id}"] a[href$="/event/${game.game_id}"]`).filter({ visible: true }).first().click();
}

// Event Detail (7.1c) at 390 px, reached in-app from the Sportsbook with the slip emptied: of
// `games` (a card's games by start), the first not started that lists both a spread and a total,
// else any line, else the first. The live card (side 1 = home on the left, side 2 = away, V-1),
// the winner market's outcome cards (2, or 3 with X for soccer; lowest green, highest purple;
// never the word "odds"), the Staked Sentiment card (a split or "Too few traders yet", Pool
// Participation), the share button and "Select to bet"; a tap then the other side (one leg for
// the game, the CTA turns to "Bet • …"); each More markets kind the card lists: opened, its lines
// in pairs, a pick that replaces the game's leg in place (D-353), its label; then the CTA opens
// the slip (through the slip pill) and the slip is emptied.
async function eventScreen(s, games, nav) {
  const { page, check, has } = s;
  const kindsOf = (g) => [...new Set((g.markets ?? []).filter((m) => m.picks?.length).map((m) => m.kind))];
  const pre = (g) => !g.in_play;
  const ev = games.find((g) => pre(g) && kindsOf(g).includes("spread") && kindsOf(g).includes("total"))
    ?? games.find((g) => pre(g) && kindsOf(g).some((k) => k !== "winner")) ?? games.find(pre) ?? games[0];
  if (!ev) return fail(s.pass, "event", "no game on the card");
  // The slip pill (the way to the drawer's Clear) shows on the Sportsbook, not on Account's bets, where
  // the live pass stands with the slip steps' three picks still in (7.2a-r).
  await nav("Sports");
  await page.waitForSelector(".home-game .pick");
  await clearSlip(page);
  await openEvent(page, nav, ev);
  await page.waitForSelector("#event-teams");
  await page.waitForSelector("#event-split, #event-thin", { timeout: 10000 }).catch(() => {});
  const wanted = kindsOf(ev).filter((k) => k === "spread" || k === "total");
  const tabs = await page.locator("#event-tabs .ev-more-tab").evaluateAll((cs) => cs.map((c) => c.id.replace("event-tab-", "")));
  const winner = (ev.markets ?? []).find((m) => m.kind === "winner")?.picks ?? ev.picks ?? [];
  await check("event", {
    ...(await eventBasics(page, winner.length === 3 ? 3 : 2)),
    sentiment: (await has("#event-split .k-sentiment[role='img'][aria-label]")) || (await has("#event-thin")),
    pool: LIVE || (await has("#event-pool #event-traders")),
    selectToBet: (await page.locator("#event-cta").innerText()).trim() === "Select to bet",
    moreCollapsed: !(await has("#event-lines")),
    tabsAsCard: wanted.every((k) => tabs.includes(k)),
    // Team logos in the header circles (31 Oct): any shown loads; otherwise the sport icon.
    badgeLogos: await page.evaluate(async () => {
      const imgs = [...document.querySelectorAll("#event-teams .ev-badge.logo img")];
      await Promise.all(imgs.map((i) => (i.complete ? null : new Promise((r) => { i.onload = r; i.onerror = r; setTimeout(r, 4000); }))));
      return imgs.every((i) => i.naturalWidth > 0) && document.querySelectorAll("#event-teams .ev-badge").length === 2;
    }),
  });
  const oneLeg = async () => (await eventSlipCount(page)) === 1;
  const cards = page.locator("#event-outcomes .ev-pick");
  const n = await cards.count();
  await cards.nth(0).click();
  await ctaHolds(page, true);
  await cards.nth(n - 1).click();
  await page.waitForSelector(`#event-outcomes .ev-pick:nth-child(${n})[aria-pressed='true']`);
  await check("event-picked", {
    oneLeg: await oneLeg(),
    otherSide: (await cards.nth(0).getAttribute("aria-pressed")) === "false",
    ctaBet: /^Bet • (Total \d+\.\d\d|1 Pick)$/.test((await page.locator("#event-cta").innerText()).trim()),
  });
  for (const k of wanted) {
    await page.locator(`#event-tab-${k}`).click();
    await page.waitForSelector(`#event-tab-${k}[aria-pressed='true']`);
    await page.waitForSelector("#event-lines .ev-line .ev-pick");
    const line = page.locator("#event-lines .ev-line").first().locator(".ev-pick:not([disabled])").first();
    const label = (await line.getAttribute("aria-label")) ?? "";
    await line.click();
    await page.waitForSelector("#event-lines .ev-pick[aria-pressed='true']");
    await check(`event-${k}`, {
      lines: (await page.locator("#event-lines .ev-line").count()) >= 1,
      twoSides: await page.locator("#event-lines .ev-line").evaluateAll((ls) => ls.every((l) => l.querySelectorAll(".ev-pick").length === 2)),
      oneLeg: await oneLeg(),
      labelled: k === "total" ? /^(Over|Under) \d/.test(label) : /[+−]\d/.test(label),
      noOddsWord: !/\bodds\b/i.test(label),
      winnerOff: (await page.locator("#event-outcomes .ev-pick[aria-pressed='true']").count()) === 0,
    });
  }
  if (!wanted.length) {
    report.push({ scheme: "dark", pass: s.pass, name: "event-lines", ok: true, note: "the card lists no spread or total (replay tape or the demo fixtures)" });
  }
  // The CTA opens the slip as the slip pill does (a route or a drawer); then the slip is emptied.
  const from = new URL(page.url()).pathname;
  await page.locator("#event-cta").click();
  const opened = await page.waitForFunction((p) => location.pathname !== p || !!document.querySelector("[role='dialog']"), from, { timeout: 10000 })
    .then(() => true).catch(() => false);
  await check("event-cta", { opensSlip: opened });
  await clearSlip(page);
}

// What every event page shows (7.1c): the live card with side 1 (home) on the left and side 2
// (away) on the right, the share button, `n` outcome cards 1 / X / 2 in that order toned (lowest
// green, highest purple, else neutral; screen_checks.mjs leaves `.k-outcome-card` to this), and
// never the word "odds". 31 Oct (user request): no filler words: no "HOME • [1]" roles, no
// "OUTCOME 1" labels, no visible market title, no PREMATCH badge, no "Tip-off" / "Kick-off".
// The drawer's coin selector (31 Oct): USDC and SOL (when listed) show their logo, the euro its
// glyph; its own menu (not the system's select) lists every coin with its mark, symbol and
// name, the current one ticked, and closes on Escape. Returns to EUR.
async function coinLogos(page) {
  const logo = async () => page.locator("#stake .k-currency img.k-currency-badge").getAttribute("src").catch(() => null);
  const eur = (await logo()) === null;
  await page.locator("#stake-currency").click();
  await page.waitForSelector("#stake-currency-list");
  const menu = await page.locator("#stake-currency-list [role=option]").evaluateAll((os) => os.map((o) => ({
    code: o.dataset.code,
    marked: !!o.querySelector(".k-currency-badge"),
    named: !!o.querySelector(".k-coin-opt-name")?.textContent.trim(),
    selected: o.getAttribute("aria-selected") === "true",
  })));
  const noSelect = (await page.locator("#stake select").count()) === 0;
  await page.screenshot({ path: `${out}/m390-coin-menu.png` }).catch(() => {});
  await page.keyboard.press("Escape");
  await page.waitForSelector("#stake-currency-list", { state: "detached" });
  await pickCoin(page, "stake-currency", "USDC");
  await page.waitForSelector("#stake .k-currency img[src='/coins/usdc.png']");
  const usdcOk = await page.waitForFunction(() => {
    const i = document.querySelector("#stake .k-currency img[src='/coins/usdc.png']");
    return !!i && i.complete && i.naturalWidth > 0;
  }, null, { timeout: 5000 }).then(() => true, () => false);
  await pickCoin(page, "stake-currency", "EUR");
  await page.waitForSelector("#stake .k-currency span.k-currency-badge");
  const menuOk = noSelect && menu.length >= 2 && menu.every((o) => o.marked && o.named)
    && menu.filter((o) => o.selected).map((o) => o.code).join() === "EUR";
  return eur && usdcOk && menuOk;
}

async function eventBasics(page, n) {
  const names = await page.locator("#event-teams .ev-team-name").allInnerTexts();
  const cards = await page.locator("#event-outcomes .k-outcome-card").evaluateAll((cs) => cs.map((c) => ({
    id: c.id,
    label: !!c.querySelector(".k-outcome-card-label"),
    value: Number(c.querySelector(".k-outcome-card-value")?.textContent.trim()) || 0,
    tone: /\btone-(lowest|highest|neutral)\b/.exec(c.className)?.[1] ?? null,
  })));
  const priced = cards.map((c) => c.value).filter((v) => v > 0);
  const [lo, hi] = [Math.min(...priced), Math.max(...priced)];
  const want = (v) => (v > 0 && lo < hi && v === lo ? "lowest" : v > 0 && lo < hi && v === hi ? "highest" : "neutral");
  const codes = n === 3 ? ["1", "X", "2"] : ["1", "2"];
  return {
    liveCard: names.length === 2 && (await page.locator("#event-teams .ev-team-role").count()) === 0,
    outcomes: cards.length === n && cards.every((c, i) => !c.label && c.id === `event-outcome-${codes[i].toLowerCase()}`),
    // What is seen: the screen-reader-only market title is left out.
    noFiller: !/\b(outcome \d|prematch|tip-off|kick-off|moneyline|match winner|full time result)\b/i.test(await page.evaluate(() => {
      const c = document.querySelector("#event").cloneNode(true);
      c.querySelectorAll(".sr-only").forEach((e) => e.remove());
      return c.textContent;
    })),
    outcomesToned: cards.every((c) => c.tone === want(c.value)),
    noOddsWord: !/\bodds\b/i.test(await page.locator("#event").innerText()),
    share: await page.locator("#event-share").isVisible(),
    backButton: (await page.locator(".k-subheader .k-icon-btn").count()) > 0,
  };
}

// A soccer game (V-2, 7.0b) on the event page: "Full Time Result (1-X-2)", three outcome cards,
// the draw picked as the game's one leg and tapped out again. Reached from the Soccer tab.
async function soccerEvent(s, game, nav) {
  const { page, check } = s;
  await clearSlip(page);
  await openEvent(page, nav, game);
  await page.waitForSelector("#event-outcomes");
  const basics = await eventBasics(page, 3);
  const draw = page.locator("#event-outcome-x");
  await draw.click();
  await page.waitForSelector("#event-outcome-x[aria-pressed='true']");
  await ctaHolds(page, true);
  await check("event-soccer", {
    ...basics,
    // The market's name stays for screen readers only.
    title: /Full Time Result \(1-X-2\)/.test(await page.locator("#event-market-title").textContent()),
    drawCard: /Draw/.test(await draw.innerText()),
    drawPicked: (await eventSlipCount(page)) === 1,
  });
  await draw.click();
  await page.waitForSelector("#event-outcome-x[aria-pressed='false']");
  await ctaHolds(page, false);
}

// Event Detail's CTA (7.1c, D-505): "Select to bet" on an empty slip, "Bet • …" once it holds
// picks. Waits for `picks` (true: some, false: none).
const ctaHolds = (page, picks) => page.waitForFunction((want) => {
  const t = document.querySelector("#event-cta")?.innerText.trim() ?? "";
  return want ? /^Bet •/.test(t) : t === "Select to bet";
}, picks, { timeout: 10000 });

// The slip's pick count on Event Detail, which may hide the slip pill behind its own CTA (7.2a,
// D-505): the pill's count, read even while hidden; with no pill in the page at all, the bet
// drawer's picks through the CTA (then closed, back on the event page).
async function eventSlipCount(page) {
  const count = page.locator("#slip-bar .k-slip-pill-count");
  if (await count.count()) return Number(/\d+/.exec(await count.textContent())?.[0] ?? 0);
  if (!/^Bet •/.test((await page.locator("#event-cta").innerText().catch(() => "")).trim())) return 0;
  const from = new URL(page.url()).pathname;
  await page.locator("#event-cta").click();
  await page.waitForSelector("#bet-drawer #slip-picks", { timeout: 10000 });
  const n = await page.locator("#bet-drawer #slip-picks .k-pick").count();
  await page.keyboard.press("Escape");
  await page.waitForSelector("#bet-drawer", { state: "detached" });
  if (new URL(page.url()).pathname !== from) await go(page, from);
  await page.waitForSelector("#event-cta");
  return n;
}

// The bottom nav of a 390 px page, by route (e2e_ui.mjs `nav`: "Home" is Sports, 7.0a); House
// is a section of Account and Live is gone (V-7, 7.1f). A sub-page (receipt, Confirm) has a back
// button instead of the nav: it is taken first.
const navOf = (page) => (label) => navTo(page, label);

// L8-4: `/me` (the My Bets screen until then) redirects to Account's bets: the URL becomes
// /account#bets and `#bets` stands in the viewport, just under the sticky app bar and ticker
// (at its top when the page is tall enough to scroll that far).
async function meRedirect(page) {
  await page.waitForURL("**/account#bets", { timeout: 10000 });
  await page.waitForSelector("#bets");
  await page.waitForTimeout(2000);
  return page.evaluate(() => {
    const top = document.querySelector("header.app-top")?.getBoundingClientRect().bottom ?? 0;
    const r = document.querySelector("#bets").getBoundingClientRect();
    const room = document.documentElement.scrollHeight - innerHeight;
    const scrolledToIt = Math.abs(r.top - top) <= 16 || scrollY >= room - 2;
    return { url: location.pathname + location.hash === "/account#bets", inView: r.top >= top - 2 && r.top < innerHeight && scrolledToIt };
  });
}

// The cash-out celebration (7.1f, guidelines §8) on a design build: the bets' two taps (the offer,
// then the confirm) open it on the design data's cashed-out bet (D-530). The payout in € and
// ≈ SOL, View Transaction (purple, Solscan) above Brag on 𝕏 (dark, tweet intent), and nothing
// else (31 Oct, user request: no settled balance, honesty line, closed-bet row or verification
// card); Back closes it onto Settled.
// The celebrations' motion (31 Oct, user request), in a context with motion on: the confetti
// burst, the figure counting up (seen below its value early, at its value once done), the
// staggered rise; then the cash-out's burst. `url` is the design build.
async function celebrationMotion(url) {
  const s = await session("design390", 390, { reducedMotion: "no-preference" });
  const { page, check } = s;
  await guarded(s, async () => {
  await page.goto(url + "/confirm");
  await page.waitForSelector("#celebration #celebrate-amount .k-payout-eur");
  const early = (await page.locator("#celebrate-amount .k-payout-eur").textContent()).trim();
  const confetti = await page.locator("#celebration .cel-confetti span").count();
  const rising = await page.locator("#celebration .cel-stream > .k-celebrate").evaluate((e) => getComputedStyle(e).animationName);
  await page.waitForFunction(() => document.querySelector("#celebrate-amount .k-payout-eur")?.textContent.trim() === "€113.75", null, { timeout: 4000 }).catch(() => {});
  await page.waitForTimeout(2200);
  await check("celebration-motion", {
    confetti: confetti >= 24,
    countsUp: early !== "€113.75" && /^€[\d,]+\.\d\d$/.test(early),
    lands: (await page.locator("#celebrate-amount .k-payout-eur").textContent()).trim() === "€113.75",
    labelled: (await page.locator("#celebrate-amount .k-payout-eur").getAttribute("aria-label")) === "€113.75",
    rises: rising === "cel-rise",
  });
  await page.goto(url + "/me");
  await page.waitForSelector("#bets .mb-card button.mb-cashout");
  // One tap (1 Nov): the offer is signed at once.
  await page.locator(".mb-card button.mb-cashout").first().click();
  await page.waitForSelector("#cashout-celebration #celebrate-amount");
  const cashConfetti = await page.locator("#cashout-frame .cel-confetti span").count();
  await page.waitForTimeout(2200);
  await check("cashout-motion", {
    confetti: cashConfetti >= 24,
    lands: /^€[\d,]+\.\d\d$/.test((await page.locator("#cashout-frame .k-payout-eur").textContent()).trim()),
  });
  });
  return s.rows;
}

async function cashOutCelebration(s, url) {
  const { page, check, has } = s;
  // An old /me link lands on Account's bets (L8-4).
  await page.goto(url + "/me");
  const me = await meRedirect(page);
  await check("me-redirect", { ...me, underFaucet: await page.locator("#account-faucet + #bets").count() === 1 });
  await page.waitForSelector("#bets .mb-card button.mb-cashout");
  await page.locator(".mb-card button.mb-cashout").first().click();
  await page.waitForSelector("#cashout-celebration #celebrate-amount");
  const text = await page.locator("#cashout-celebration").innerText();
  const buttons = await page.locator("#cashout-frame .cel-actions > *").evaluateAll((xs) => xs.map((x) => ({
    id: x.id, cls: x.className, href: x.getAttribute("href") ?? "",
  })));
  await check("cashout-celebration", {
    title: /Cash Out Secured!/.test(text),
    payout: /Total Payout Received/i.test(text) && /≈ [\d.]+ SOL/.test(await page.locator("#celebrate-amount").innerText()),
    noExtras: !(await has("#cashout-balance, #cashout-proof, .cx-match, .k-celebrate-sub, .k-txbadge"))
      && !/mock-filled|test money|Settled Balance|Solscan Transaction|Signature|Slot|Cashed out/i.test(text),
    order: buttons.length === 2 && buttons[0].id === "celebrate-tx" && !/secondary/.test(buttons[0].cls)
      && buttons[0].href.startsWith("https://solscan.io/tx/") && buttons[1].id === "celebrate-brag"
      && /secondary/.test(buttons[1].cls) && buttons[1].href.startsWith("https://x.com/intent/post"),
    truth: !/slippage|#892|instantly routed|guaranteed/i.test(text),
  });
  await page.locator("#cashout-close").click();
  await page.waitForSelector("#cashout-celebration", { state: "detached" });
  if (!(await has("#bets #my-bets-tabs"))) fail(s.pass, "cashout-close", "Back did not return to the bets");
}

// ---------------------------------------------------------------------------------------------
// 390 px: every screen.
// ---------------------------------------------------------------------------------------------
{
  const s = await session("m390", 390);
  const { page, check, has, shown } = s;
  await guarded(s, async () => {
    const nav = navOf(page);
    // No honesty pill on any screen (31 Oct).
    const noPill = (f) => f.pills.length === 0;
    const placeText = async () => (await page.locator("#gioca").innerText()).trim();
    // The slip's price-moved rule (D-361): while the banner shows, Bet waits for Accept.
    const movedRule = async () => !(await shown("#price-moved")) || await page.locator("#gioca").isDisabled();

    await page.goto(base + "/");
    await page.waitForSelector(".home-game .pick");
    // The Sportsbook (7.1a): tabs, live and upcoming cards, 1 / X / 2, event links; toned (generic).
    const homeChecks = async () => ({
      ...(await sportsbookChecks(page)),
      slipBarHidden: !(await shown("#slip-bar")),
    });
    let f = await check("home", await homeChecks());
    if (!noPill(f)) fail("m390", "home-pill", `honesty pill on Home: ${JSON.stringify(f.pills)}`);
    const rows = await homeRows(page);
    for (const p of gatePicks(rows)) {
      await page.locator(".home-game").nth(p.row).locator(`.pick-${p.which}`).click();
    }
    // 6.1c, 7.0a: the slip pill carries the slip's quoted total ("3 Picks • Total 5.94").
    await page.waitForSelector("#slip-bar .k-slip-pill-total", { timeout: 20000 });
    await check("home-picked", {
      threePicked: (await page.locator(".pick.selected").count()) === 3,
      slipBarOdds: /^3 Picks\s*•\s*Total \d+\.\d\d/.test((await page.locator("#slip-bar").innerText()).replace(/\s+/g, " ").trim()),
    });

    // The bet drawer (7.1b): the pill opens it over the Sportsbook (the URL stays "/").
    const text = async (sel) => (await page.locator(sel).first().innerText()).trim();
    await page.locator("#slip-bar").click();
    await page.waitForSelector("#bet-drawer #gioca:not([disabled])", { timeout: 20000 });
    await check("slip", {
      overSportsbook: new URL(page.url()).pathname === "/" && (await page.locator("#home").count()) === 1,
      threeLegs: (await page.locator("#slip-picks .k-pick").count()) === 3,
      picksBadge: (await text("#bet-drawer .k-badge.picks")) === "3 picks",
      tonedPicks: (await page.locator("#slip-picks .k-pick-value.tone-lowest").count()) >= 1
        && (await page.locator("#slip-picks .k-pick-value.tone-highest").count()) >= 1,
      removeEach: (await page.locator("#slip-picks .k-pick-remove").count()) === 3,
      stakeBlock: (await text("#stake-title")) === "Stake amount" && (await text("#stake .k-currency-code")) === "EUR",
      presets: (await page.locator("#stake-presets button").count()) === 4
        && (await page.locator("#stake-presets button").allInnerTexts()).join(" ") === "€10 €25 €50 €100",
      slider: await has("#stake-slider"),
      total: /^\d+\.\d\d$/.test(await text("#slip-total")),
      payout: /^€[\d,]+\.\d\d$/.test(await text("#slip-payout")),
      onePlaceBet: (await page.locator("#gioca").count()) === 1 && /^Bet • €10\.00$/.test(await placeText()),
      // 31 Oct (user request): no fee, sentiment or "Pay with" links under the summary, no copy-fee
      // sentence under the tip switch; SOL and USDC wear their logos in the coin selector.
      noFiller: !(await has("#fee-details, #others-toggle, #pay-token, #slip-details"))
        && !/fee included|What others think|Pay with|you earn 80%/i.test(await page.locator("#bet-drawer").innerText()),
      coinLogos: await coinLogos(page),
      pillHidden: !(await shown("#slip-bar")),
      noOddsWord: !/\bodds\b/i.test(await page.locator("#bet-drawer").innerText()),
      movedRule: await movedRule(),
    });
    // The slider runs from the minimum stake (€5); a preset sets it and the CTA follows.
    await page.locator("#stake-presets button", { hasText: /^€25$/ }).click();
    await page.waitForSelector("#gioca:not([disabled]) >> text=Bet • €25.00", { timeout: 20000 });
    await check("slip-min-stake", {
      minBound: (await page.locator("#stake-slider").getAttribute("min")) === "5"
        && /Min: €5/.test(await text("#stake .k-stake-bounds")),
      presetPressed: (await text("#stake-presets button[aria-pressed='true']")) === "€25",
    });
    await page.locator("#stake-presets button", { hasText: /^€10$/ }).click();
    await page.waitForSelector("#gioca:not([disabled]) >> text=Bet • €10.00", { timeout: 20000 });
    // Paying in a token besides mock USDC and mockSOL (BONK in the coin selector, 31 Oct; always
    // in demo mode): its typed amount, the swap fee line, stake = the swap's min_out, tips off;
    // then back to euros.
    const payToken = await page.waitForSelector("#stake-currency[data-codes~='BONK']", { state: "attached", timeout: LIVE ? 3000 : 10000 }).then(() => true, () => false);
    if (!LIVE && !payToken) fail("m390", "slip-token", "no BONK in the coin selector in demo mode");
    if (payToken) {
      {
        await pickCoin(page, "stake-currency", "BONK");
        await page.waitForSelector("#token-amount");
        await page.fill("#token-amount", "10000");
        await page.waitForSelector("#swap-fee", { timeout: 20000 });
        const stakeShown = await text("#swap-stake");
        await check("slip-token", {
          swapFee: /^Swap fee /.test(await text("#swap-fee")),
          stakeIsMinOut: LIVE ? /^[\d,.]+ USDC$/.test(stakeShown) : stakeShown === "9.88 USDC",
          payLine: LIVE || /10,000 mockBONK/.test(await text("#swap-fee")),
          tipOff: (await page.locator("#share-tip").isDisabled()) && !(await page.locator("#share-tip").isChecked()),
          tipWhy: await has("#tip-why >> text=staked in test USDC"),
          onePlaceBet: (await page.locator("#gioca").count()) === 1,
          placeWords: /^Bet • 10,000 mockBONK$/.test(await placeText()),
        });
      }
      await page.locator("#pay-eur").click();
      await page.waitForSelector("#gioca:not([disabled]) >> text=Bet • €10.00", { timeout: 20000 });
      const back = await page.locator("#stake-presets button[aria-pressed='true']").innerText().catch(() => "");
      if (back.trim() !== "€10" || (await has("#swap-fee"))) fail("m390", "slip-token-back", `stake ${back}`);
      // 7.2a-r: "Pay in euros" goes with the token block; the focus must stay in the drawer, or
      // Escape no longer closes it (the live gate of 30 Oct timed out there).
      if (!(await page.evaluate(() => !!document.activeElement?.closest("#bet-drawer")))) {
        fail("m390", "slip-token-back", "the focus left the drawer: Escape would not close it");
      }
    }

    if (LIVE) {
      // No wallet: Bet would open one. Account's bets ask to connect.
      await page.keyboard.press("Escape");
      await page.waitForSelector("#bet-drawer", { state: "detached" });
      await bets(page);
      await page.waitForSelector("#bets >> text=Connect a wallet to see your bets.");
      f = await check("bets-no-wallet");
    } else {
      // Share as a tip (every example game starts after the example card's time), then Bet: the
      // sheet shows the ring and the steps ticking off (1 Nov), then the celebration opens.
      const tipAllowed = !(await page.locator("#share-tip").isDisabled());
      await page.locator("#share-tip").check();
      await page.waitForSelector("#share-tip:checked");
      await check("slip-tip", { tipAllowed, tipOn: await page.locator("#share-tip").isChecked(), noTipSentence: !(await has("#tip-why")) });
      await page.locator("#gioca").click();
      await page.waitForSelector("#bet-drawer .slip-placing", { state: "attached" });
      await page.waitForSelector("#place-steps [data-step='escrowed'].done", { timeout: 15000 });
      await check("slip-placing", {
        ring: await has("#bet-drawer .k-ring"),
        steps: (await page.locator("#place-steps .k-seq-step").count()) >= 3,
        escrowedDone: await has("#place-steps [data-step='escrowed'].done"),
        oneCurrent: (await page.locator("#place-steps .k-seq-step.current").count()) <= 1,
        noPicksList: !(await has("#bet-drawer #slip-picks")),
      });
      await page.waitForURL("**/confirm", { timeout: 20000 });
      await page.waitForSelector("#celebration #celebrate-slip");
      const brag = (await page.locator("#celebrate-brag").getAttribute("href")) ?? "";
      const celebText = await page.locator("#celebration").innerText();
      await check("celebration", {
        // 31 Oct (user request): no tx badge or slot, honesty line, slip id or kind line.
        noExtras: !(await has("#celebration .k-txbadge, #celebration .k-celebrate-sub, #celebrate-kind, .cel-slip-id"))
          && !/Tx Confirmed|Slot|mock-filled|test money|Slip #|Shared as a tip/i.test(celebText),
        title: /Schedina Locked In!/.test(celebText),
        payout: /^€[\d,]+\.\d\d$/.test(await text("#celebrate-amount .k-payout-eur")),
        legs: (await page.locator("#celebrate-slip .k-leg-row").count()) >= 1,
        twoButtons: (await page.locator("#celebration .cel-actions .k-cta").count()) === 2
          && /View Transaction/.test(await text("#celebrate-tx")) && /^Brag on\s*X?$/.test(await text("#celebrate-brag")),
        bragIntent: /^https:\/\/x\.com\/intent\/post\?text=.+&url=http/.test(brag),
        nothingElse: !/Track|Sportsbook|Settlement/i.test(celebText),
        noOddsWord: !/\bodds\b/i.test(celebText),
        backHeader: await has(".k-subheader .k-icon-btn"),
      });

      // Account's bets (7.1f; L8-4: a section of Account under the Faucet, no tab of its own):
      // the open bet's card, its legs with their status, the live cash-out value and "Cash out €x".
      await bets(page);
      await page.waitForSelector("#bets .mb-card");
      await check("bets", {
        tabs: await has("#bets #my-bets-tabs"),
        title: (await page.locator("#bets-title").innerText()).trim().toLowerCase() === "your bets",
        legs: (await page.locator(".mb-card .mb-leg .k-leg-row").count()) >= 2,
        legStatus: /Starts|Live|Won|Lost/.test(await page.locator(".mb-card .mb-legs").first().innerText()),
        value: await has(".mb-card .mb-cash-value .mb-cash-eur"),
        cashOut: /^Cash out €/.test((await page.locator(".mb-card button.mb-cashout").first().innerText()).trim()),
        receiptLink: await has(".mb-card a.receipt-link"),
      });
      // One tap (1 Nov, superseding D-102's two): the desk's signed offer is signed at once, no
      // confirm step (demo mode stops at the signing step with its demo words).
      const cq = fixture("cashout_quote.json");
      await page.locator(".mb-card .mb-cashout").first().click();
      await page.waitForSelector(".mb-card .mb-cash-note.mb-error >> text=Demo data", { timeout: 5000 })
        .catch(() => fail("m390", "bets-signing", "the tap did not reach the signing step"));
      await check("bets-offer", {
        oneTap: !(await has(".mb-card .mb-cashout-confirm, .mb-card .mb-offer")),
        usdcShown: (await page.locator(".mb-card .mb-cash-usdc").first().innerText()).trim() === usdc(cq.c_u),
      });
      await page.locator("#my-bets-tabs button", { hasText: "Settled" }).click();
      await page.waitForSelector(".mb-list.settled .mb-settled");
      await check("bets-settled", {
        result: await has(".mb-settled .mb-result"),
        paid: await has(".mb-settled .mb-paid[data-paid-u]"),
        receipt: await has(".mb-settled a.receipt-link"),
      });
      await page.locator("#my-bets-tabs button", { hasText: "Open" }).click();
      await page.waitForSelector("#bets .mb-card");

      // Receipt of a plain bet: legs, fills marked mock-filled, every transaction on Solscan
      // (V-9), no copy link.
      await page.locator(".mb-card a.receipt-link").first().click();
      await page.waitForSelector("#receipt");
      await check("receipt", {
        legs: (await page.locator("#receipt-legs .mb-leg").count()) >= 1,
        mockFills: await has("#receipt-mock") && (await page.locator("#receipt .rc-fills .rc-tag", { hasText: "Mock-filled" }).count()) >= 1,
        txLinks: (await page.locator("#receipt-txs a[href^='https://solscan.io/tx/']").count()) >= 2,
        noExplorer: (await page.locator("#receipt a[href*='explorer.solana.com']").count()) === 0,
        noCopy: !(await has("#copy-link")),
      });
      // The receipt's back goes to Account's bets (L8-4: through /me to /account#bets), the
      // Account tab marked.
      await page.locator(".k-subheader-start .k-icon-btn").first().click();
      await check("receipt-back", {
        ...(await meRedirect(page)),
        accountTab: (await page.locator("#bottom-nav a[aria-current='page']").innerText()).trim() === "Account",
      });

      // Tipsters (L9, user request 1 Nov): the tipsters only, each with its 30D ROI and win
      // rate, best 30-day return first, everyone with a tip (no orders, no "Show everyone").
      await nav("Tipsters");
      await page.waitForSelector("#board .tp-row");
      const rows = await page.locator("#board .tp-row").count();
      const rois = await page.locator("#board .tp-row-roi .tp-row-value").allInnerTexts();
      const pct = (t) => Number(t.replace("−", "-").replace(/[+%]/g, ""));
      await check("tipsters", {
        everyone: rows === fixture("tipsters.json").tipsters.length,
        roi30d: rois.length === rows,
        winRate: (await page.locator("#board .tp-row-win .tp-row-value").count()) === rows,
        bestFirst: rois.every((r, i) => i === 0 || pct(rois[i - 1]) >= pct(r)),
        noOrders: !(await has("#tipster-sorts, #tipsters-all")),
      });

      // A tipster's page: the header card (stats, socials, Solscan), then the open parlays only:
      // title, copies, overall stake, at most three logos, total; no chart, no docked CTA.
      const tipsterData = fixture("tipster.json");
      await page.locator(`#board a.tp-row-link[href$="/tipsters/${tipsterData.tipster.wallet}"]`).click();
      await page.waitForSelector("#tipster-head");
      await page.waitForSelector("#open-tips .tp-parlay, #open-tips-none");
      await check("tipster", {
        header: await has("#tipster-head #tp-roi") && await has("#tipster-solscan[href^='https://solscan.io/account/']"),
        parlays: (await page.locator("#open-tips .tp-parlay").count()) === tipsterData.open.length,
        rowParts: await has("#open-tips .tp-parlay .tp-parlay-title") && await has("#open-tips .tp-parlay .tp-parlay-copies")
          && await has("#open-tips .tp-parlay .tp-parlay-staked") && await has("#open-tips .tp-parlay .tp-parlay-total"),
        logosAtMostThree: await page.locator("#open-tips .tp-logos").evaluateAll((ls) => ls.every((l) => l.querySelectorAll(".tp-logo:not(.more)").length <= 3)),
        noRecord: !(await has(".chart-svg, #settled-tips, #tip-dock")),
      });
      await page.locator("#follow").click();
      await page.waitForSelector("#follow-note >> text=Demo data");

      // A parlay (/copy/<id>): the games as the Sportsbook's bars, the tipster's pick selected
      // and the others locked; "Copied by" opens the copiers sheet.
      await page.locator("#open-tips .tp-parlay").first().click();
      await page.waitForSelector("#parlay-head");
      const tipId = new URL(page.url()).pathname.split("/").pop();
      const tip = tipsterData.open.find((t) => t.ticket === tipId) ?? tipsterData.open[0];
      const bars = page.locator("#parlay-legs .pl-leg");
      const locked = await bars.evaluateAll((ls) => ls.every((l) => {
        const mine = l.querySelectorAll(".k-outcome.selected");
        const others = [...l.querySelectorAll(".k-outcome:not(.selected)")];
        return mine.length === 1 && others.every((b) => b.disabled);
      }));
      await check("parlay", {
        legs: (await bars.count()) === tip.legs.length,
        locked,
        tipster: (await page.locator("#parlay-tipster").getAttribute("href")) === `/tipsters/${tipsterData.tipster.wallet}`,
        copiedBy: /^Copied by [\d,]+$/.test((await page.locator("#parlay-copiers").innerText()).replace(/\s+/g, " ").trim()),
        copyCta: (await page.locator("#tip-copy").innerText()).replace(/\s+/g, " ").trim().startsWith("Copy Schedina"),
        noOddsWord: !/\bodds\b/i.test(await page.locator("#parlay").innerText()),
      });
      // A locked tap changes nothing (the Sportsbook's slip stays as it was).
      const slipBefore = await page.evaluate(() => document.querySelectorAll(".k-outcome.selected").length);
      await page.locator("#parlay-legs .k-outcome:not(.selected)").first().click({ force: true }).catch(() => {});
      await check("parlay-locked-tap", { unchanged: (await page.evaluate(() => document.querySelectorAll(".k-outcome.selected").length)) === slipBefore });
      await page.locator("#parlay-copiers").click();
      await page.waitForSelector("#copiers-sheet");
      await check("parlay-copiers", {
        title: /^Copiers \(\d/.test(await page.locator("#copiers-sheet .k-sheet-title").innerText()),
        list: await has("#copiers-sheet .k-copier, #copiers-empty"),
        solscan: await page.locator("#copiers-sheet a.k-copier").evaluateAll((as) => as.every((a) => a.href.startsWith("https://solscan.io/tx/"))),
      });
      await page.locator("#copiers-sheet .k-sheet-close").click();
      await page.waitForSelector("#copiers-sheet", { state: "detached" });

      // A locked game (/copy/<id>/event/<game>): the event page with the tipster's pick fixed;
      // its CTA is Copy Schedina.
      await page.locator("#parlay-legs .k-match-head").first().click();
      await page.waitForSelector("#event-locked");
      await check("parlay-event", {
        path: new RegExp(`^/copy/${tipId}/event/`).test(new URL(page.url()).pathname),
        onePick: (await page.locator("#event-outcomes .k-outcome-card.selected, #event-outcomes [aria-pressed='true']").count()) === 1,
        cta: (await page.locator("#event-cta").innerText()).replace(/\s+/g, " ").trim() === "Copy Schedina",
      });
      await page.goBack();
      await page.waitForSelector("#parlay-head");

      // Copy Schedina: the bet drawer in copy mode (the tip's picks locked, no CLEAR, no tip
      // switch, the 2% copy fee as its own row), the stake slider and coin pill of a bet; €10,
      // Copy, the progress in the CTA, then the placement celebration on /confirm (demo stream).
      await page.locator("#tip-copy").click();
      await page.waitForSelector("#bet-drawer");
      await page.waitForSelector("#bet-drawer #gioca:not([disabled])", { timeout: 10000 }).catch(() => {});
      const drawerCta = async () => (await page.locator("#bet-drawer #gioca").innerText()).replace(/\s+/g, " ").trim();
      await check("copy", {
        title: (await page.locator("#bet-drawer .k-sheet-title").innerText()).trim() === "Copy Schedina",
        picks: (await page.locator("#slip-picks .slip-pick").count()) === tip.legs.length,
        lockedPicks: (await page.locator("#slip-picks button[aria-label^='Remove']").count()) === 0,
        noClear: !(await has("#bet-drawer .k-sheet-clear")),
        noTipSwitch: !(await has("#bet-drawer #share-tip")),
        copyFee: /^€\d+\.\d\d$/.test((await page.locator("#slip-copy-fee").innerText()).trim()),
        stakeBlock: (await has("#bet-drawer #stake")) && (await has("#bet-drawer #stake-currency")),
        cta: /^Copy Schedina • €\d+\.\d\d$/.test(await drawerCta()),
      });
      await stakeTen(page);
      await page.waitForSelector("#bet-drawer #gioca:not([disabled]) >> text=Copy Schedina • €10.00", { timeout: 10000 });
      await page.locator("#bet-drawer #gioca").click();
      await page.waitForURL("**/confirm", { timeout: 20000 });
      await page.waitForSelector("#celebration #celebrate-amount", { timeout: 20000 });
      await check("copy-placed", {
        payout: await has("#celebration #celebrate-amount"),
        viewTx: await has("#celebration #celebrate-tx"),
      });

      // A settled tip's receipt: one result transaction per game; the open tip's receipt links
      // "Copy this schedina" to its parlay (6.1d).
      await go(page, "/ticket/" + tipsterData.settled[0].ticket);
      await page.waitForSelector("#receipt");
      await check("receipt-settled", {
        results: (await page.locator("#receipt-txs li", { hasText: "Result posted" }).count()) >= 1,
        noCopy: !(await has("#copy-link")),
      });
      await go(page, "/ticket/" + tipId);
      await page.waitForSelector("#receipt");
      const copyHref = (await has("#copy-link")) ? await page.locator("#copy-link").getAttribute("href") : null;
      await check("receipt-tip", { copyLink: copyHref === `/copy/${tipId}` });
    }

    // Event Detail (7.1c), opened in-app (More markets when the card lists spreads and totals).
    await eventScreen(s, byStart((LIVE ? s.card() : fixture("card.json"))?.games ?? []), nav);

    // Account (31 Oct, user request): "Your wallet" (its worth in euros only), "Faucet" (the
    // drawer's coin pill with USDC and BONK, no SOL; the chosen coin's balance; the airdrop,
    // off without a wallet in demo), then "Your bets" (L8-4), nothing else. The house and the profile editor have their
    // own pages (/house, /profile). No link to the removed Live screen.
    await nav("Account");
    await page.waitForSelector("#account-faucet");
    const sections = await page.locator("#account > section").evaluateAll((xs) => xs.map((x) => x.id));
    const coins = await coinCodes(page, "faucet-coin");
    await pickCoin(page, "faucet-coin", "BONK").catch(() => {});
    const bonkLabel = (await page.locator("#faucet-get").innerText().catch(() => "")).trim();
    await pickCoin(page, "faucet-coin", "USDC");
    const usdcLabel = (await page.locator("#faucet-get").innerText()).trim();
    f = await check("account", {
      sections: sections.join(" ") === "account-wallet account-faucet bets",
      titles: (await page.locator("#account-wallet-title").innerText()).trim() === "Your wallet"
        && /^Faucet/i.test((await page.locator("#account-faucet-title").innerText()).trim()),
      euroOnly: !(await has("#account-wallet .k-usdc, #account-wallet .acct-sol, #account-balances, #account-house, #account-profile, #mock-label-account")),
      coins: coins.join(" ") === "USDC BONK",
      coinLabels: /^Get 1,000\.00 USDC$/.test(usdcLabel) && /^Get 50,000 mockBONK$/.test(bonkLabel),
      coinLogo: (await page.locator("#account-faucet .k-currency img").getAttribute("src")) === "/coins/usdc.png",
      faucetOffInDemo: LIVE || await page.locator("#faucet-get").isDisabled(),
      testChip: /Test tokens/i.test(await page.locator("#account-faucet .acct-test-chip").innerText()),
      noLive: (await page.locator("a[href='/live']").count()) === 0,
    });
    // The house's numbers and the profile editor on their own pages; /live is no screen.
    await page.goto(base + "/house");
    await page.waitForSelector("#account-house #house-hedger", { timeout: 10000 }).catch(() => {});
    await check("house", {
      house: await has("#account-house #house-vault") && await has("#house-hedger")
        && await has("#house-fees >> text=Builder revenue") && (LIVE || await has("#house-mock")),
      // Regression (31 Oct): the page scrolls with the wheel; body's overflow-x and overscroll
      // rules used to stop every wheel and swipe from reaching it.
      scrolls: await (async () => {
        await page.evaluate(() => scrollTo(0, 0));
        await page.mouse.move(195, 400);
        await page.mouse.wheel(0, 500);
        await page.waitForTimeout(300);
        const y = await page.evaluate(() => scrollY);
        await page.evaluate(() => scrollTo(0, 0));
        return y > 0;
      })(),
    });
    await page.goto(base + "/profile");
    await page.waitForSelector("#account-profile", { timeout: 10000 }).catch(() => {});
    await check("profile-edit", {
      profile: await has("#account-profile #profile-handle") && (await page.locator("#account-profile input[id^='profile-']").count()) === 5,
    });
    await page.goto(base + "/live");
    await page.waitForTimeout(500);
    if (await has("#live-picks") || await has("#ticker.lv-ticker")) fail("m390", "live-gone", "the Live screen still renders");
    await page.goto(base + "/account");
    await page.waitForSelector("#account-faucet");
    // An old /me link (the My Bets screen until L8-4) lands on Account's bets.
    await page.goto(base + "/me");
    await check("me-redirect", await meRedirect(page));
    // No screen shows the honesty pill any more (31 Oct: it left Account with the rest).
    for (const r of s.rows) {
      if (r.pills.length) fail("m390", `${r.name}-pill`, `honesty pill on ${r.name}: ${r.pills}`);
    }
    // The dev wallet (dev builds, D-150), reached from Account: nothing is made or sent here.
    if (await has("#account-burner")) {
      await page.locator("#account-burner").click();
      await page.waitForSelector("#burner");
      await check("dev-wallet", { burnerButton: await page.locator("#burner").isVisible() });
    } else {
      fail("m390", "dev-wallet", "no link to the dev wallet on Account (a build without dev tools?)");
    }
  });
}

// The system set to light: still the dark theme (D-310, dark only).
{
  const s = await session("m390", 390, { colorScheme: "light" });
  await guarded(s, async () => {
    await s.page.goto(base + "/");
    await s.page.waitForSelector(".home-game .pick");
    await s.check("home-light-preference");
  });
}

// ---------------------------------------------------------------------------------------------
// 1024 and 1440 px: the desktop composition, then every top-level screen and the sub-pages.
// ---------------------------------------------------------------------------------------------
const desktop = [];
const firstGame = byStart(fixture("card.json").games)[0].game_id;
const tipster = fixture("tipster.json");
const SUBPAGES = LIVE ? [] : [
  `/tipsters/${tipster.tipster.wallet}`, `/copy/${tipster.open[0].ticket}`, `/ticket/${fixture("tickets.json").tickets[0].ticket}`,
];
for (const width of [1024, 1440]) {
  const s = await session(`d${width}`, width, { height: 900, scale: 1 });
  const { page, check, has, shown, pass } = s;
  await guarded(s, async () => {
    // V-7: one centred column of at most 480 px, the app bar (balance in euros) on top-level
    // screens, the bottom nav, no side columns; no honesty pill (31 Oct).
    const chrome = async (top = true) => page.evaluate(({ top, live }) => {
      const vis = (sel) => !!document.querySelector(sel)?.checkVisibility();
      const app = document.querySelector("div.app")?.getBoundingClientRect();
      const pills = [...document.querySelectorAll(".k-honesty")].filter((e) => e.checkVisibility());
      return {
        column: !!app && app.width <= 480 && Math.abs(app.left + app.width / 2 - innerWidth / 2) < 2,
        // Live (no wallet, 7.2a-r): the app bar's connect button stands where the balance pill
        // shows once a wallet is connected. User request (31 Oct): no DEPOSIT button, and the pill
        // is one euro figure: no TEST chip, no SOL beside it.
        appBar: !top || (vis(".k-appbar .k-appbar-brand") && !vis(".k-appbar #deposit") && vis('.k-appbar [aria-label="Account"]')
          && (live ? vis(".k-appbar .wallet") && !vis("#balance") : vis("#balance") && !vis("#balance .k-test-chip") && !vis("#balance .k-balance-sol"))
          && vis("#ticker-bar")),
        bottomNav: !top || [...document.querySelectorAll("#bottom-nav a")].map((a) => a.innerText.trim()).join(",") === "Sports,Tipsters,Account",
        noSideColumns: !vis("aside.app-slip") && !document.querySelector(".k-topbar, aside.app-side"),
        noPill: pills.length === 0,
      };
    }, { top, live: LIVE });
    await page.goto(base + "/");
    await page.waitForSelector(".home-game .pick");
    await check("home", { ...(await chrome()), noPill: !(await shown("#slip-bar")) });
    // Three picks: the slip pill shows the count and the quoted total.
    const rows = await homeRows(page);
    for (const p of gatePicks(rows)) await page.locator(".home-game").nth(p.row).locator(`.pick-${p.which}`).click();
    await page.waitForSelector("#slip-bar .k-slip-pill-total", { timeout: 20000 });
    await check("home-slip", {
      ...(await chrome()),
      slipPill: /^3 Picks\s*•\s*Total \d+\.\d\d/.test((await page.locator("#slip-bar").innerText()).replace(/\s+/g, " ").trim()),
    });
    for (const path of ["/tipsters", "/account", `/event/${LIVE ? (byStart(s.card()?.games ?? [])[0]?.game_id ?? firstGame) : firstGame}`, ...SUBPAGES]) {
      await go(page, path);
      await page.waitForURL("**" + path);
      await page.waitForSelector("main.app-main > *");
      const sel = desktopReady(path, LIVE);
      await page.waitForSelector(sel, { timeout: 10000 }).catch(() => fail(pass, path, `no ${sel}`));
      const name = path.replace(/^\//, "").replace(/\/.*/, "") || "home";
      // Sub-pages (event, copy, receipt) wear the back header instead of the app bar and nav.
      const top = !/^\/(event|copy|ticket)\//.test(path);
      await check(name, {
        ...(await chrome(top)),
        ...(top ? {} : { backHeader: await shown(".k-subheader .k-icon-btn") }),
      });
    }
  });
  desktop.push(...s.rows);
}

// ---------------------------------------------------------------------------------------------
// DESIGN_URL: the bet drawer on the mockup's seeded slip and the placement celebration on the
// mockup's ticket (7.1b), at 390 and 1440 px, the event page's spread and total tabs on the
// design card, and (390 px) the cash-out celebration (7.1f).
// ---------------------------------------------------------------------------------------------
const design = [];
if (DESIGN_URL) {
  for (const width of [390, 1440]) {
    const s = await session(`design${width}`, width, { height: 900, scale: width < 1000 ? 2 : 1 });
    const { page, check, has } = s;
    await guarded(s, async () => {
      const text = async (sel) => (await page.locator(sel).first().innerText()).trim();
      await page.goto(DESIGN_URL + "/");
      await page.waitForSelector("#slip-bar");
      await page.locator("#slip-bar").click();
      await page.waitForSelector("#bet-drawer #gioca", { timeout: 10000 });
      // D-492 (7.2a): the seeded slip may carry a total other than the quote's, so the price-moved
      // banner shows: Bet must wait for Accept while it does (D-361), then the quote's figures.
      const banner = await page.waitForSelector("#price-moved", { timeout: 3000 }).then(() => true, () => false);
      const gated = !banner || await page.locator("#gioca").isDisabled();
      if (banner) {
        await check("drawer-price-moved", { gated, accept: await has("#price-moved .k-moved-accept") });
        await page.locator("#price-moved .k-moved-accept").click();
      }
      await page.waitForSelector("#bet-drawer #gioca:not([disabled])", { timeout: 10000 });
      // The seeded legs (fixtures/design/slip.json) toned across the slip (lowest green, highest
      // purple), and the quote's Total and Payout (fixtures/design/quote.json; €113.75 as drawn).
      const seeded = JSON.parse(readFileSync(new URL("../fixtures/design/slip.json", import.meta.url), "utf8"));
      const quote = JSON.parse(readFileSync(new URL("../fixtures/design/quote.json", import.meta.url), "utf8"));
      const legValues = seeded.picks.map((p) => oddsText(p.odds_u));
      const values = (await page.locator("#slip-picks .k-pick-value").allInnerTexts()).map((v) => v.trim());
      const nums = values.map(Number);
      await check("drawer-seeded", {
        picks: values.join(" ") === legValues.join(" "),
        details: (await page.locator("#slip-picks .k-pick-detail").allInnerTexts()).join(" / ")
          === "vs Fenerbahçe / vs Olympiacos / vs NY Knicks",
        toned: Number(await text("#slip-picks .k-pick-value.tone-highest")) === Math.max(...nums)
          && Number(await text("#slip-picks .k-pick-value.tone-lowest")) === Math.min(...nums),
        stake: (await text("#stake .k-stake-big")) === "€25.00",
        approx: (await text("#stake .k-stake-approx")) === "≈ 0.184 SOL (1 SOL = €135.80)",
        total: (await text("#slip-total")) === oddsText(quote.odds_offered_u),
        payout: (await text("#slip-payout")) === "€113.75",
        bet: (await text("#gioca")) === "Bet • €25.00",
        noBanner: !(await page.locator("#price-moved").isVisible().catch(() => false)),
      });
      await page.goto(DESIGN_URL + "/confirm");
      await page.waitForSelector("#celebration #celebrate-slip", { timeout: 10000 });
      const ticket = JSON.parse(readFileSync(new URL("../fixtures/design/ticket.json", import.meta.url), "utf8"));
      await check("celebration-mockup", {
        payout: (await text("#celebrate-amount .k-payout-eur")) === "€113.75",
        sol: /≈ 0\.838 SOL/.test(await text("#celebrate-amount .k-payout-chip")),
        stake: /€25\.00/.test(await text("#celebrate-stake")),
        total: /4\.55/.test(await text("#celebrate-total")) && /3 legs/.test(await text("#celebrate-total")),
        legs: (await page.locator("#celebrate-slip .k-leg-row").count()) === 3,
        solscan: (await page.locator("#celebrate-tx").getAttribute("href")) === `https://solscan.io/tx/${ticket.tx.open_pending}?cluster=devnet`,
      });
      // The design card lists spreads and totals (the demo card has winners only) and a soccer
      // 1-X-2 game: the event page's More markets and its three outcomes at 390 px.
      if (width < 1000) {
        const card = JSON.parse(readFileSync(new URL("../fixtures/design/card.json", import.meta.url), "utf8"));
        // The Sportsbook on the mockup's content (7.1a): the live card (its score, 31 Oct), the
        // upcoming cards, then the Soccer tab's 1–X–2 card and its draw.
        await go(page, "/");
        await page.waitForSelector("#home .home-game .pick");
        const count = async () => Number((/^(\d+) Picks?/.exec((await page.locator("#slip-bar").innerText().catch(() => "")).trim()) ?? [0, 0])[1]);
        const sel = (game, c) => page.locator(`.home-game[data-game-id="${game}"] .pick-${c}`);
        const pressed = async (game, c) => (await sel(game, c).getAttribute("aria-pressed")) === "true";
        const hero = await page.evaluate(() => ({
          names: [...document.querySelectorAll("#home-hero .k-match-name")].map((e) => e.innerText.trim()).join(" / "),
          values: [...document.querySelectorAll("#home-hero .k-outcome-value")].map((e) => e.innerText.trim()).join(" "),
          score: [...document.querySelectorAll("#home-hero .k-match-value")].map((e) => e.innerText.trim()).join(" "),
          clock: document.querySelector("#home-hero .k-match-league")?.innerText.trim() ?? "",
        }));
        await check("sportsbook", {
          ...(await sportsbookChecks(page)),
          liveHero: hero.names === "Panathinaikos / Fenerbahçe" && hero.values === "1.96 1.62",
          // The live score and match clock (31 Oct): the fixture's 61-66 in the third quarter.
          liveScore: hero.score === "61 66" && / • Q3 4:12$/.test(hero.clock),
          seededSlip: (await count()) === 3,
          // Team logos (31 Oct): Celtics and Knicks have one; EuroLeague's Panathinaikos does not,
          // so the hero keeps its code circle.
          designLogos: await page.evaluate(() => {
            const src = [...document.querySelectorAll("#home .k-match-logo img")].map((i) => i.getAttribute("src"));
            return src.includes("/teams/nba/boston-celtics.png") && src.includes("/teams/nba/new-york-knicks.png")
              && !!document.querySelector("#home-hero .k-match-code");
          }),
          // The sport chips' counters (31 Oct): the seeded slip's three basketball picks.
          sportCount: (await page.locator("#sport-basketball-count").innerText().catch(() => "")) === "3"
            && (await page.locator("#sport-soccer-count").count()) === 0,
        });
        await page.locator("#sport-soccer").click();
        await page.waitForSelector("#sport-soccer[aria-pressed='true']");
        const ucl = "ucl-ars-bay-2026-10-29";
        const before = await count();
        await sel(ucl, "draw").click();
        await page.waitForSelector(`.home-game[data-game-id="${ucl}"] .pick-draw[aria-pressed="true"]`);
        const drawAdded = (await count()) === before + 1;
        await sel(ucl, "away").click();
        await page.waitForSelector(`.home-game[data-game-id="${ucl}"] .pick-away[aria-pressed="true"]`);
        await check("sportsbook-soccer", {
          ...(await sportsbookChecks(page)),
          oneCard: (await page.locator("#home .home-game").count()) === 1,
          drawAdded,
          awayReplacesDraw: !(await pressed(ucl, "draw")) && (await count()) === before + 1,
        });
        await sel(ucl, "away").click();
        await page.waitForSelector(`.home-game[data-game-id="${ucl}"] .pick-away[aria-pressed="false"]`);
        await page.locator("#sport-basketball").click();
        await eventScreen(s, byStart(card.games), navOf(page));
        // L9: @lorenzo_hoops's page, the Master Treble parlay and its copiers sheet, as drawn.
        const tip = JSON.parse(readFileSync(new URL("../fixtures/design/tipster.json", import.meta.url), "utf8"));
        await go(page, `/tipsters/${tip.tipster.wallet}`);
        await page.waitForSelector("#open-tips .tp-parlay");
        await check("design-tipster", {
          pro: await has("#tipster-head .k-badge.pro"),
          stats: (await page.locator("#tp-roi .k-stat-value").innerText()) === "+24.8%" && (await page.locator("#tp-copiers .k-stat-value").innerText()) === "1,284",
          links: (await page.locator("#tipster-links a[href^='https://']").count()) === 4,
          parlays: (await page.locator("#open-tips .tp-parlay").count()) === 3,
          first: (await page.locator("#open-tips .tp-parlay").first().getAttribute("href")) === `/copy/${tip.open[0].ticket}`
            && (await page.locator("#open-tips .tp-parlay-title").first().innerText()) === "Master Treble"
            && (await page.locator("#open-tips .tp-parlay-copies").first().innerText()) === "1,284 copies",
        });
        await page.locator("#open-tips .tp-parlay").first().click();
        await page.waitForSelector("#parlay-copiers");
        await check("design-parlay", {
          title: (await page.locator("#parlay-title").innerText()) === "Master Treble",
          copiedBy: (await page.locator("#parlay-copiers").innerText()).replace(/\s+/g, " ").trim() === "Copied by 1,284",
          logos: (await page.locator("#parlay-head .tp-logo img").count()) >= 1,
        });
        await page.locator("#parlay-copiers").click();
        await page.waitForSelector("#copiers-sheet .k-copier");
        await check("design-copiers", {
          title: (await page.locator("#copiers-sheet .k-sheet-title").innerText()) === "Copiers (1,284)",
          rows: (await page.locator("#copiers-sheet a.k-copier[href^='https://solscan.io/tx/'][href$='?cluster=devnet']").count()) === 6,
          volume: (await page.locator("#copiers-volume").innerText()) === "€28,450 Pool Volume",
          undimmed: await page.locator("#copiers-sheet").evaluate(() => getComputedStyle(document.querySelector(".tp-copiers-layer .k-sheet-backdrop")).backgroundColor === "rgba(0, 0, 0, 0)"),
        });
        await page.locator("#copiers-sheet .k-sheet-close").click();
        const soccer = card.games.find((g) => (g.markets ?? []).some((m) => m.kind === "winner" && m.picks?.length === 3));
        if (soccer) await soccerEvent(s, soccer, navOf(page));
        else fail(s.pass, "event-soccer", "the design card lists no 1-X-2 game");
        await cashOutCelebration(s, DESIGN_URL);
      }
    });
    design.push(...s.rows);
  }
  design.push(...(await celebrationMotion(DESIGN_URL)));
}

await browser.close();
console.log(JSON.stringify(report, null, 1));
const rows = report.filter((r) => r.width !== undefined);
const mobile = rows.filter((r) => r.pass === "m390");
const n = (xs) => `${xs.filter((r) => r.ok).length}/${xs.length}`;
const sum = (k) => rows.reduce((a, r) => a + (r[k] ?? 0), 0);
console.log(`PHONE GATE ${failed ? "FAILED" : "OK"} (${LIVE ? "live" : "demo"}): 390 px ${n(mobile)} screens; `
  + `desktop ${n(desktop)} at 1024/1440 px (480 px column, app bar, nav)`
  + (DESIGN_URL ? `; design build ${n(design)} (price moved, Sportsbook, spread and total tabs, tipster, cash-out celebration)` : "")
  + ` — dark, no horizontal scroll, no trading words, no honesty pill, ${sum("pairs")} markets toned, ${sum("figures")} figures in tabular Plus Jakarta Sans`
  + `${failed ? `; failed: ${report.filter((r) => r.ok === false || r.errors).map((r) => `${r.pass}/${r.name ?? "errors"}`).join(" ")}` : ""}`);
process.exit(failed ? 1 : 0);
