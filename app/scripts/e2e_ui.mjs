// The UI steps devnet_e2e.mjs drives, one place for their selectors (6.2a-g; v2 screens by
// 7.2a-g). e2e_dry.mjs runs the same steps on a demo and a design build (no chain, nothing
// signed), so a screen that drops an id the devnet run needs fails there first. Every step
// navigates in-app: a page load would drop the in-tab burner wallet (D-230). Selectors are ids,
// classes and routes, never a label's words, except where the words are the check.
import { prettyLabel, sportOf } from "./slate.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/// Polls `fn` once a second until it returns a truthy value; throws after `secs`.
export async function until(what, fn, secs) {
  const end = Date.now() + secs * 1000;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timeout (${secs} s): ${what}`);
    await sleep(1000);
  }
}

/// The bottom nav's tabs (7.0a; L8-4: Sports, Tipsters, Account) by route, so a label change
/// never breaks a run. "Home" is the Sports tab (its L6 name). My Bets is no tab since L8-4: its
/// bets are Account's `#bets` section ([`bets`]).
export const NAV_HREF = { Home: "/", Sports: "/", Tipsters: "/tipsters", Account: "/account" };

/// Account's bets (L8-4, was the My Bets tab): the Account tab, then its `#bets` section (the
/// Open / Settled switch `#my-bets-tabs`, the cards, the cash-out) scrolled into view.
export async function bets(p) {
  await nav(p, "Account");
  const section = p.locator("#bets");
  await section.waitFor({ timeout: 10000 });
  await section.scrollIntoViewIfNeeded();
}

/// The bottom nav of a 390 px page, by the tab's route; a sub-page (Confirm, a receipt, a copy
/// page, the event page) has a back button instead: take it first. An open bet drawer closes.
export async function nav(p, label) {
  const href = NAV_HREF[label];
  if (!href) throw new Error(`no bottom-nav tab "${label}"`);
  if (await p.locator("#bet-drawer").isVisible().catch(() => false)) {
    await p.keyboard.press("Escape");
    await p.waitForSelector("#bet-drawer", { state: "detached" });
  }
  if (!(await p.locator("#bottom-nav").isVisible())) {
    await p.locator(".k-subheader-start .k-icon-btn").filter({ visible: true }).first().click();
    await p.waitForSelector("#bottom-nav");
  }
  await p.locator(`#bottom-nav a[href="${href}"]`).click();
  await p.waitForURL((u) => new URL(u).pathname === href, { timeout: 10000 });
}

/// The Sportsbook's tab for `sport` ("basketball" | "soccer"; 7.1a: each tab lists only its
/// sport, D-480), pressed when it is not. A card of one sport may draw no tabs at all.
export async function showSport(p, sport) {
  const tab = p.locator(`#sport-${sport}`);
  if (!(await tab.count())) return;
  if ((await tab.getAttribute("aria-pressed")) !== "true") {
    await tab.click();
    await p.waitForSelector(`#sport-${sport}[aria-pressed="true"]`);
  }
}

/// The game ids the Sportsbook shows under every sport tab (the slate as a person can pick it),
/// ending on the first tab.
export async function shownGames(p) {
  const ids = () => p.locator(".home-game").evaluateAll((gs) => gs.map((g) => g.dataset.gameId));
  const tabs = await p.locator("#sport-tabs .k-sport-tab").evaluateAll((ts) => ts.map((t) => t.id.replace(/^sport-/, "")));
  if (!tabs.length) return ids();
  const all = [];
  for (const s of tabs) {
    await showSport(p, s);
    all.push(...(await ids()));
  }
  await showSport(p, tabs[0]);
  return all;
}

/// A winner pick on the Sportsbook (7.1a: the live card or an upcoming card, by game id; 1 =
/// `.pick-home`, X = `.pick-draw`, 2 = `.pick-away`), pressed, on the game's sport tab. `game` is
/// a `/card` game.
export async function pickWinner(p, game, which) {
  await showSport(p, sportOf(game));
  const pick = `.home-game[data-game-id="${game.game_id}"] .pick-${which}`;
  await p.locator(pick).click();
  await p.waitForSelector(`${pick}[aria-pressed="true"]`);
}

/// A spread or total (slate.mjs `lineLeg`) from the game's event page, reached from the
/// Sportsbook by the card's event link: the kind under "More markets", then the line's button by
/// its label (7.1c: the aria-label is "<label>, <value>", event.rs LinePair). Ends on the event
/// page; `openSlip` goes back to the Sportsbook (the event page may hide the pill, D-505).
export async function pickLine(p, game, line) {
  await showSport(p, sportOf(game));
  await p.locator(`.home-game[data-game-id="${game.game_id}"] a[href$="/event/${game.game_id}"]`).filter({ visible: true }).first().click();
  await p.waitForSelector("#event-teams");
  await p.locator(`#event-tab-${line.kind}`).click();
  await p.waitForSelector(`#event-tab-${line.kind}[aria-pressed="true"]`);
  const button = `#event-lines .ev-pick[aria-label^="${prettyLabel(line.label)}, "]`;
  await p.locator(button).click();
  await p.waitForSelector(`${button}[aria-pressed="true"]`);
}

/// "3 Picks" or "1 Pick", as the slip pill counts (7.0a).
export const picksText = (n) => (n === 1 ? "1 Pick" : `${n} Picks`);

/// The slip pill's words from its parts (7.0a `SlipPill`: the count, then "Total <b>x</b>"), so
/// its icon and "SCHEDINA ›" never matter: "3 Picks • Total 5.94", or "3 Picks" before a price.
export async function pillText(p) {
  const count = (await p.locator("#slip-bar .k-slip-pill-count").innerText()).trim();
  const total = p.locator("#slip-bar .k-slip-pill-total b");
  return (await total.count()) ? `${count} • Total ${(await total.innerText()).trim()}` : count;
}

/// The bet drawer (7.1b: a BottomSheet over the Sportsbook) through the slip pill, from the
/// Sportsbook (or any screen: it goes there first when the pill is not shown), once the desk has
/// priced the slip (its Total shows). Returns the pill's words (`pillText`).
export async function openSlip(p) {
  if (!(await p.locator("#slip-bar").isVisible().catch(() => false))) await nav(p, "Sports");
  await p.waitForSelector("#slip-bar .k-slip-pill-total", { timeout: 30000 });
  const bar = await pillText(p);
  await p.locator("#slip-bar").click();
  await p.waitForFunction(() => /^\d+\.\d\d$/.test(document.querySelector("#bet-drawer #slip-total")?.textContent.trim() ?? ""), null, { timeout: 30000 });
  return bar;
}

/// Empties the slip through the drawer's CLEAR (it closes).
export async function clearSlip(p) {
  if (!(await p.locator("#bet-drawer").isVisible().catch(() => false))) {
    if (!(await p.locator("#slip-bar").isVisible())) return;
    await p.locator("#slip-bar").click();
    await p.waitForSelector("#bet-drawer");
  }
  await p.locator("#sheet-clear").click();
  await p.waitForSelector("#bet-drawer", { state: "detached" });
}

/// The drawer keeps the last ticket's way to pay (7.2a-r: E's drawer opened in mockBONK after C,
/// with no presets and no currency pill): "Pay in euros" (#pay-eur) first, when it shows. A
/// mockSOL stake keeps the stake block (no #pay-eur), so it is left alone.
export async function leaveTokenMode(p) {
  if (await p.locator("#pay-eur").isVisible()) {
    await p.locator("#pay-eur").click();
    await p.waitForSelector("#stake-presets");
  }
}

/// A stake preset of the drawer's stake block (`#stake-presets`, €10 €25 €50 €100), pressed (the
/// default, pressed again so a run never depends on it). By position, the words being euros.
export async function stakePreset(p, euros) {
  const i = [10, 25, 50, 100].indexOf(euros);
  if (i < 0) throw new Error(`no €${euros} preset`);
  await leaveTokenMode(p);
  const b = p.locator("#stake-presets button").nth(i);
  await b.click();
  await p.waitForFunction((i) => document.querySelectorAll("#stake-presets button")[i]?.getAttribute("aria-pressed") === "true", i);
}
export const stakeTen = (p) => stakePreset(p, 10);

/// A coin on a coin pill (kit::CoinPill, 31 Oct: its own menu, not a select): the pill `#id`
/// carries the coin in `data-code` and every coin in `data-codes`; its menu `#id-list` lists
/// each as `[data-code]`. Opens the menu and taps `code` (nothing when it is already chosen).
export async function pickCoin(p, id, code) {
  if ((await p.locator(`#${id}`).getAttribute("data-code")) === code) return;
  await p.locator(`#${id}`).click();
  await p.locator(`#${id}-list [data-code="${code}"]`).click();
}

/// The coins a pill offers (`data-codes`), e.g. ["EUR", "SOL", "USDC", "BONK"].
export const coinCodes = async (p, id) =>
  ((await p.locator(`#${id}`).getAttribute("data-codes").catch(() => null)) ?? "").split(" ").filter(Boolean);

/// The drawer's currency pill (7.1b: the stake block's `#stake-currency`, EUR · SOL · USDC;
/// SOL pays in mockSOL through mock-amm, V-3).
export async function payIn(p, code) {
  await leaveTokenMode(p);
  await pickCoin(p, "stake-currency", code);
  await p.waitForFunction((c) => document.querySelector("#stake-currency")?.dataset.code === c, code);
}

/// The coin pill's BONK (31 Oct: it replaced the "Pay with mockBONK" link): the token block
/// with its typed amount.
export async function payInToken(p, code = "BONK") {
  await leaveTokenMode(p);
  await pickCoin(p, "stake-currency", code);
  await p.waitForSelector("#token-amount");
}

/// The drawer's CTA words ("Bet • €10.00", "Bet • 0.0736 SOL", "Bet • 10,000 mockBONK").
export const ctaText = async (p) => (await p.locator("#gioca").innerText()).replace(/\s+/g, " ").trim();

/// The drawer waits for Accept when a price moved against the user (D-361; a better one is taken
/// silently): accept, until Bet is on. Returns how many moves were accepted.
export async function readyToPlace(p, secs = 60) {
  let accepted = 0;
  await until("Bet enabled", async () => {
    if (await p.locator("#price-moved").isVisible()) {
      await p.locator("#price-moved .k-moved-accept").click();
      await p.waitForSelector("#price-moved", { state: "detached" }).catch(() => {});
      accepted++;
    }
    return p.locator("#gioca").isEnabled();
  }, secs);
  return accepted;
}

/// What the drawer shows of the desk's answers (31 Oct: no fee table behind a link any more):
/// `total` (#slip-total, "4.55"), and while paying in a token, the swap line's `paid` ("10,000
/// mockBONK") and `stake` (#swap-stake, "9.88 USDC"); `null` where not shown.
export async function drawerShown(p) {
  const text = async (sel) => (await p.locator(sel).first().textContent({ timeout: 1000 }).catch(() => null))?.trim() ?? null;
  const swap = await text("#swap-fee");
  return {
    total: await text("#slip-total"),
    paid: (swap && /\u00b7 (.+) stakes /.exec(swap)?.[1]) ?? null,
    stake: await text("#swap-stake"),
  };
}

/// Account's test-USDC balance as `usdc()` prints it ("1,000.00 USDC"). The card shows the
/// wallet's worth in euros only (31 Oct); the USDC balance rides in `data-usdc-u`.
export async function accountUsdc(p) {
  const u = await p.locator("#account-balance").getAttribute("data-usdc-u");
  if (u === null) throw new Error("Account has no USDC balance yet");
  const n = Number(u);
  const group = (x) => x.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${group(Math.floor(n / 1e6))}.${String(Math.floor((n % 1e6) / 1e4)).padStart(2, "0")} USDC`;
}

/// Client-side navigation (the router's history), so an in-tab burner wallet survives.
export const go = (p, path) => p.evaluate((x) => {
  history.pushState(null, "", x);
  dispatchEvent(new PopStateEvent("popstate"));
}, path);

/// Account's faucet (31 Oct): the coin (`USDC`, `BONK`) on its pill, the airdrop, its
/// celebration (`#faucet-celebration`, View Transaction on the explorer), closed again.
/// Returns the celebration's View Transaction link.
export async function faucet(p, coin = "USDC", timeout = 120000) {
  await pickCoin(p, "faucet-coin", coin);
  await p.locator("#faucet-get:not([disabled])").click();
  await p.waitForSelector("#faucet-celebration #celebrate-tx[href]", { timeout });
  const href = await p.locator("#faucet-celebration #celebrate-tx").getAttribute("href");
  await p.locator("#faucet-close").click();
  await p.waitForSelector("#faucet-celebration", { state: "detached" });
  return href;
}

/// A bet's entry in Account's bets (`#bets`) for a ticket PDA: a parlay card or a single's row while open, a row under
/// Settled once closed.
export const ticketCard = (p, pda) => p.locator(`li.ticket[id="ticket-${pda}"]`);

/// A celebration's View Transaction (guidelines §8): `#celebrate-tx` in the shared frame (7.1b).
/// `root` is the celebration's id ("celebration", "cashout-celebration"); a copy celebrates on
/// /confirm like any bet (L9).
export const viewTx = (p, root) => p.locator(`#${root} #celebrate-tx`).first();

/// A Solscan devnet transaction link (V-9): `https://solscan.io/tx/<sig>?cluster=devnet`; the
/// signature, or null.
export const solscanSig = (href) => href?.match(/^https:\/\/solscan\.io\/tx\/([1-9A-HJ-NP-Za-km-z]{60,90})\?cluster=devnet$/)?.[1] ?? null;
