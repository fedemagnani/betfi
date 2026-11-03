// Dry run of devnet_e2e.mjs's UI path (6.2a-g; v2 screens by 7.2a-g): the same in-app steps
// (e2e_ui.mjs) and the same selectors, on builds without a chain, so a screen change that breaks
// the devnet run shows up before any SOL is spent. Nothing is signed or sent: every request off
// the app's own origin is aborted, and demo builds stop at their scripted celebration.
//
//   PLAYWRIGHT_CORE=… node scripts/e2e_dry.mjs <demo-build-url> [<design-build-url>]
//
// On each build: (design) the seeded slip in the drawer → the dev wallet makes a burner (in
// memory) → Account (wallet card, Deposit, balance) → the Sportsbook: the slate under every sport
// tab, the devnet run's winner picks by game id, each on its sport's tab (a soccer pick under
// Soccer), plus (design build: the card has lines) ticket B's spread or total from the event
// page (slate.mjs lineLeg) → the slip pill ("n Picks • Total x") opens the bet drawer (7.1b: the
// legs, the line's label) → the stake block (#stake-presets €10, "Bet • €10.00"; the currency
// pill in USDC and back) → the fee table's rows → Share as a tip → a moved price accepted → Bet
// (status in the CTA) → the placement celebration (View Transaction) → (demo build) "Pay with
// mockBONK" with its swap rows → Account's bets (#bets: a card by ticket id, its cash-out offer and confirm;
// Settled with result, paid amount and closing transaction) → Tipsters (everyone, the tipster,
// the active parlay, the docked "Copy Schedina • Total x") → Copy confirm (€5 on the slider, the
// fee line, "Copy Schedina • €5.00") → the celebration on /copy → Account's house section. The
// last line is a one-line summary; exits 1 on any failed step. Paying in mockSOL (devnet_e2e's
// E2E_SOL) needs a desk listing mockSOL, which neither build has: its pill option is checked in
// the source (slate_check.mjs), not here.
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import * as ui from "./e2e_ui.mjs";
import { byStart, LINE_GAME, lineLeg, planTickets, prettyLabel } from "./slate.mjs";

const { chromium } = await import(process.env.PLAYWRIGHT_CORE ?? "playwright-core");
const urls = process.argv.slice(2);
if (!urls.length) {
  console.error("usage: node scripts/e2e_dry.mjs <demo-build-url> [<design-build-url>]");
  process.exit(2);
}
const CHROME = process.env.CHROME_PATH
  ?? `${homedir()}/Library/Caches/ms-playwright/chromium_headless_shell-1208/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const json = (dir, f) => JSON.parse(readFileSync(new URL(`../fixtures/${dir}/${f}`, import.meta.url), "utf8"));
// The app's formatting (app/src/format.rs): 4550000 → "4.55".
const odds = (u) => `${Math.floor(u / 1e6)}.${String(Math.floor((u % 1e6) / 1e4)).padStart(2, "0")}`;
const browser = await chromium.launch({ executablePath: CHROME });
const results = [];

for (const base of urls) {
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, colorScheme: "dark", reducedMotion: "reduce", serviceWorkers: "block" });
  const origin = new URL(base).origin;
  await ctx.route((u) => u.origin !== origin, (r) => r.abort());
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  let build = "?";
  const step = async (name, fn) => {
    try {
      const detail = await fn();
      results.push({ build, name, ok: detail !== false, ...(typeof detail === "string" ? { detail } : {}) });
    } catch (e) {
      // The first line names the call; the call log's last "waiting for" names the selector.
      const lines = String(e).split("\n");
      const sel = lines.find((l) => /waiting for locator/.test(l))?.trim() ?? "";
      results.push({ build, name, ok: false, detail: `${lines[0].slice(0, 160)} ${sel.slice(0, 160)}` });
      if (process.env.DRY_OUT) await page.screenshot({ path: `${process.env.DRY_OUT}/dry-${build}-${results.length}.png`, fullPage: true }).catch(() => {});
    }
  };
  const has = async (sel) => (await page.locator(sel).count()) > 0;

  // Which build: the shell's `data-fixtures="design"` (6.2a-p). A page load here is fine: the
  // burner is made after it.
  await page.goto(base + "/");
  await page.waitForSelector(".home-game .pick");
  build = (await has('[data-fixtures="design"]')) ? "design" : "demo";
  const fixtures = build === "design" ? "design" : "desk";
  const games = byStart(json(fixtures, "card.json").games);
  if (build === "design") {
    // The seeded slip (D-360) opens in the drawer, ready to place (readyToPlace accepts any move).
    await step("design drawer: #slip-bar opens #bet-drawer on the seeded slip, readyToPlace", async () => {
      await page.locator("#slip-bar").click();
      await page.waitForSelector("#bet-drawer #slip-picks .k-pick");
      const moves = await ui.readyToPlace(page, 20);
      const n = await page.locator("#slip-picks .k-pick").count();
      await page.keyboard.press("Escape");
      await page.waitForSelector("#bet-drawer", { state: "detached" });
      return n === 3 && `3 picks, ${moves} move(s) accepted`;
    });
  }

  // The dev wallet: a fresh burner, its address (devnet_e2e funds it here), reached in-app.
  await ui.nav(page, "Account");
  await page.locator("#account-burner").click();
  await page.waitForSelector("#burner");
  await step("dev wallet: #burner makes a wallet, #wallet-address shows it, #sol-refresh/#sol-balance", async () => {
    await page.locator("#burner").click();
    await page.waitForSelector("#wallet-address");
    const w = (await page.locator("#wallet-address").innerText()).trim();
    return /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(w) && await has("#sol-refresh") && await has("#sol-balance");
  });
  await step("Account: #account-wallet, #account-faucet (#faucet-coin, #faucet-get), #account-balance (accountUsdc)", async () => {
    await ui.nav(page, "Account");
    await page.waitForSelector("#account-wallet");
    const ok = await has("#account-faucet #faucet-coin") && await has("#faucet-get[disabled]");
    if (build === "demo") return ok ? "demo: no balance, faucet off" : false;
    const bal = await ui.accountUsdc(page);
    return ok && /^[\d,]+\.\d\d USDC$/.test(bal) && `balance ${bal}`;
  });

  // The Sportsbook: a design build seeds the mockup's slip (D-360): empty it first, as a devnet
  // run starts. The slate is every sport tab's unstarted games (a soccer game sits under Soccer,
  // D-480). Ticket B's shape when game 0 lists a line (two winners and the line, as devnet_e2e
  // plays B), else ticket A's three winners.
  let played = 0;
  await step("Sportsbook → drawer: the slate per sport tab, winners by game id on their tab, a line from the event page, the slip pill, the drawer", async () => {
    await ui.nav(page, "Sports");
    await page.waitForSelector(".home-game .pick");
    await ui.clearSlip(page);
    const shown = await ui.shownGames(page);
    const open = games.filter((g) => !g.in_play && shown.includes(g.game_id));
    const plan = planTickets(open.length);
    const line = lineLeg(open[LINE_GAME]);
    const legs = line ? plan.B : plan.A;
    for (const [i, which] of legs) await ui.pickWinner(page, open[i], which);
    if (line) await ui.pickLine(page, open[LINE_GAME], line);
    const n = legs.length + (line ? 1 : 0);
    played = n;
    const bar = await ui.openSlip(page);
    if (!new RegExp(`^${ui.picksText(n)} • Total \\d+\\.\\d\\d$`).test(bar)) return false;
    if ((await page.locator("#slip-picks .k-pick").count()) !== n) return false;
    if (line && !(await page.locator("#slip-picks").innerText()).includes(prettyLabel(line.label))) return false;
    const sports = [...new Set(legs.map(([i]) => open[i].sport ?? "basketball"))].join(" + ");
    return `${line ? "B" : "A"}: ${n} legs (${sports}${line ? `, ${line.kind} "${line.label}"` : ", no line on this card"}); pill "${bar}"`;
  });
  await step("drawer stake: #stake-presets €10 → #gioca \"Bet • €10.00\", #stake-currency USDC and back, #slip-total, #share-tip (no #tip-why while on)", async () => {
    await ui.stakeTen(page);
    await ui.until("Bet • €10.00", async () => (await ui.ctaText(page)) === "Bet • €10.00", 10);
    await ui.payIn(page, "USDC");
    const usdcCta = await ui.until("Bet in USDC", async () => { const t = await ui.ctaText(page); return /^Bet • [\d,]+\.\d\d USDC$/.test(t) && t; }, 10);
    await ui.payIn(page, "EUR");
    await ui.until("Bet • €10.00 again", async () => (await ui.ctaText(page)) === "Bet • €10.00", 10);
    const rows = /^\d+\.\d\d$/.test((await ui.drawerShown(page)).total ?? "");
    await page.locator("#share-tip").check();
    await page.waitForSelector("#share-tip:checked");
    const on = (await page.locator("#tip-why").count()) === 0;
    await page.locator("#share-tip").uncheck();
    return rows && on && `"Bet • €10.00", "${usdcCta}"`;
  });
  await step("drawer → celebration: readyToPlace, #gioca, #place-steps (ring and steps), #celebration, #celebrate-tx (Solscan devnet)", async () => {
    const moves = await ui.readyToPlace(page, 20);
    await page.locator("#gioca").click();
    await page.waitForFunction(() => !!document.querySelector("#place-steps [data-step='escrowed'].done")
      || location.pathname === "/confirm", null, { timeout: 15000 });
    await page.waitForURL("**/confirm", { timeout: 15000 });
    await page.waitForSelector("#celebration #celebrate-slip", { timeout: 15000 });
    const tx = ui.viewTx(page, "celebration");
    const href = await tx.getAttribute("href");
    // Without a signature yet the button is off (no href); with one it is a Solscan devnet link.
    const linkOk = href === null || !!ui.solscanSig(href);
    return (await tx.count()) === 1 && linkOk && (await has("#celebrate-total"))
      && `${played} legs, ${moves} price move(s) accepted${href ? ", Solscan link" : ", no signature yet"}`;
  });
  if (build === "demo") {
    await step("drawer in mockBONK: #stake-currency BONK, #token-amount, #swap-fee, #swap-stake, #pay-eur (demo: no #token-faucet)", async () => {
      await ui.nav(page, "Sports");
      await page.waitForSelector(".home-game .pick");
      await ui.pickWinner(page, games[games.length - 1], "away");
      await ui.openSlip(page);
      await ui.payInToken(page, "BONK");
      await page.fill("#token-amount", "10000");
      await page.waitForSelector("#swap-fee", { timeout: 15000 });
      const t = await ui.drawerShown(page);
      const cta = await ui.ctaText(page);
      const ok = t.paid === "10,000 mockBONK" && /USDC$/.test(t.stake ?? "") && await has("#token-line") && /^Bet • [\d,.]+ mockBONK$/.test(cta);
      // 7.2a-r: the €10 preset and the currency pill from token mode (the devnet run's E after
      // C): stakeTen and payIn leave mockBONK through #pay-eur first.
      await ui.stakeTen(page);
      const euros = await ui.until("Bet • €10.00", async () => { const t = await ui.ctaText(page); return /^Bet • €10\.00$/.test(t) && t; }, 15)
        .catch(() => ui.ctaText(page));
      await ui.payIn(page, "EUR");
      const back = await page.locator("#stake-currency").getAttribute("data-code");
      await ui.clearSlip(page);
      return ok && back === "EUR" && /^Bet • €10\.00$/.test(euros) && `"${cta}", then €10 and the pill on EUR`;
    });
  }

  // Account's bets (#bets, L8-4): the open parlay by its ticket id, its offer; a settled row's
  // closing transaction.
  const tickets = json(fixtures, "tickets.json").tickets;
  const openParlay = tickets.find((t) => t.state === "open" && t.n > 1);
  const settled = tickets.find((t) => t.state === "settled");
  await step("Account's bets: #bets li.ticket#ticket-<pda>, .mb-leg[data-status], .mb-cash-eur/-usdc, button.mb-cashout \"Cash out €x\"", async () => {
    await ui.bets(page);
    const card = ui.ticketCard(page, openParlay.ticket);
    await card.waitFor({ timeout: 10000 });
    const labels = await card.locator(".mb-leg[data-status]").count();
    const heroEur = (await card.locator(".mb-cash-eur").innerText()).trim();
    const usdcLine = (await card.locator(".mb-cash-usdc").innerText()).trim();
    const btn = (await card.locator("button.mb-cashout").innerText()).trim();
    return labels >= 2 && /USDC$/.test(usdcLine) && btn === `Cash out ${heroEur}`;
  });
  if (build === "design") {
    // One tap (1 Nov): the cash-out celebration (7.1f, D-530) with View Transaction on Solscan.
    await step("cash-out celebration: button.mb-cashout (one tap) → #cashout-celebration, #celebrate-tx (Solscan devnet), #cashout-close", async () => {
      await page.locator("button.mb-cashout").first().click();
      await page.waitForSelector("#cashout-celebration #celebrate-amount", { timeout: 15000 });
      const href = await ui.viewTx(page, "cashout-celebration").getAttribute("href");
      const paid = (await page.locator("#cashout-frame .k-payout-eur").innerText()).trim();
      await page.locator("#cashout-close").click();
      await page.waitForSelector("#cashout-celebration", { state: "detached" });
      return !!ui.solscanSig(href) && /^€[\d,]+\.\d\d$/.test(paid) && `paid ${paid}`;
    });
  }
  await step("Account's bets Settled: #my-bets-tabs, .mb-result, .mb-paid[data-paid-u], .closing a", async () => {
    await page.locator("#my-bets-tabs button", { hasText: "Settled" }).click();
    const row = ui.ticketCard(page, settled.ticket);
    await row.waitFor({ timeout: 10000 });
    const result = (await row.locator(".mb-result").innerText()).trim();
    const paid = Number(await row.locator(".mb-paid").getAttribute("data-paid-u"));
    const closing = await row.locator(".closing", { hasText: "Settled" }).count();
    const href = closing ? await row.locator(".closing a").getAttribute("href") : "";
    await page.locator("#my-bets-tabs button", { hasText: "Open" }).click();
    return /^(Won|Lost|Cashed out|Refunded)$/.test(result) && paid >= 0 && (!settled.tx?.settle || /\/tx\/\w+/.test(href)) && result;
  });

  // Tipsters → the tipster → the active parlay → the docked copy CTA → Copy confirm → Copy.
  const tipster = json(fixtures, "tipster.json");
  const tip = tipster.open[0];
  await step("Tipsters (L9): .k-appbar #balance, #board a.tp-row-link, #open-tips [data-ticket=<id>] a.tp-parlay → /copy/<id>: #parlay-head, #parlay-legs .pl-leg locked, #tip-copy", async () => {
    await ui.nav(page, "Tipsters");
    // v2 app bar (7.0a): the balance pill, or the connect button in its place.
    await page.waitForSelector(".k-appbar .k-appbar-end");
    const balance = (await page.locator(".k-appbar #balance").count()) === 1 && (await page.locator(".k-appbar .wallet").count()) === 0;
    const row = page.locator(`#board a.tp-row-link[href$="/tipsters/${tipster.tipster.wallet}"]`);
    await row.waitFor({ timeout: 10000 });
    await row.click();
    const parlay = page.locator(`#open-tips [data-ticket="${tip.ticket}"] a.tp-parlay`);
    await parlay.waitFor({ timeout: 10000 });
    const total = (await parlay.locator(".tp-parlay-total").innerText()).trim();
    await parlay.click();
    await page.waitForSelector("#parlay-head");
    const legs = await page.locator("#parlay-legs .pl-leg").count();
    const locked = await page.locator("#parlay-legs .pl-leg .k-outcome.selected").count();
    return balance && !(await has("#tip-copy-why")) && total === odds(tip.odds_offered_u) && legs === tip.legs.length
      && locked === legs && `total ${total}, ${legs} locked legs`;
  });
  await step("Copy Schedina (the drawer in copy mode): #stake-presets €10, #slip-copy-fee, #gioca \"Copy Schedina • €10.00\" → /confirm #celebration, its View Transaction", async () => {
    await page.locator("#tip-copy").click();
    await page.waitForSelector("#bet-drawer #slip-copy-fee");
    await ui.stakeTen(page);
    await page.waitForSelector("#gioca:not([disabled]) >> text=Copy Schedina • €10.00", { timeout: 10000 });
    const fee = (await page.locator("#slip-copy-fee").textContent()).trim();
    await page.locator("#gioca").click();
    // Demo builds: the example stream mints it; the placement celebration on /confirm.
    await page.waitForURL("**/confirm", { timeout: 20000 });
    await page.waitForSelector("#celebration", { timeout: 15000 });
    const tx = ui.viewTx(page, "celebration");
    const href = await tx.getAttribute("href");
    return (await tx.count()) === 1 && (href === null || !!ui.solscanSig(href)) && /^€\d+\.\d\d$/.test(fee) && `fee ${fee}`;
  });
  await step("House (/house, off Account since 31 Oct): #account-house #house-hedger", async () => {
    await ui.go(page, "/house");
    await page.waitForSelector("#account-house #house-hedger", { timeout: 10000 });
    return true;
  });
  results.push({ build, name: "no page errors", ok: errors.length === 0, ...(errors.length ? { detail: errors.slice(0, 2).join(" | ") } : {}) });
  await ctx.close();
}
await browser.close();
for (const r of results) console.log(`${r.ok ? "ok  " : "FAIL"} [${r.build}] ${r.name}${r.detail ? ` (${r.detail})` : ""}`);
const bad = results.filter((r) => !r.ok);
const builds = [...new Set(results.map((r) => r.build))].join(" + ");
console.log(`E2E DRY ${bad.length ? "FAILED" : "OK"}: ${results.length - bad.length}/${results.length} steps of devnet_e2e.mjs's UI path on the ${builds} build${urls.length > 1 ? "s" : ""} (no chain, nothing sent)`);
process.exit(bad.length ? 1 : 0);
