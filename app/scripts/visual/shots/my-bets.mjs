// L7-1 visual shots for 7.1f (l7-contracts L7-4), moved by L8-4: the bets are Account's `#bets`
// section under the Faucet (no mockup, V-7: compared with the drawn screens' style), Settled, a
// receipt, and the cash-out celebration (mockup `betfi_cash_out_celebration`) at 390 px and at
// the mockup's own frame, where it is scored. Cash-out is one tap since 1 Nov (the offer shows
// only when it pays less than the figure tapped, so it has no shot). On a design build the tap
// opens the celebration on the design data's cashed-out bet (Real
// Madrid vs Manchester City, €61.20, D-530); the plain demo build stops at "Demo data" there, so
// its celebration shots never render (design builds only). Account's top is `account-390`
// (account.mjs).
// shoot.mjs is run as `node scripts/visual/shoot.mjs <base> [out]`: the base URL is argv[2].
const base = process.argv[2];

// /account#bets (Account scrolled to its bets), then `then(page)` in the same session (clicks,
// not a reload).
const onBets = (then) => async (page) => {
  await page.goto(base + "/account#bets");
  await page.waitForSelector("#bets .mb-list");
  // The app re-aims at #bets for about a second while the cards above it load.
  await page.waitForTimeout(1500);
  if (then) await then(page);
};
const celebrate = onBets(async (p) => {
  await p.locator(".mb-card button.mb-cashout").first().click();
});

export const shots = () => [
  // The page as a "Your bets" link opens it: scrolled to #bets, under the app bar and ticker.
  { name: "account-bets-390", setup: onBets(), wait: "#bets .mb-card", vp: "m390", full: false, mockup: ["profile", "cashout"] },
  {
    name: "account-bets-settled-390",
    setup: onBets(async (p) => { await p.locator("#my-bets-tabs button", { hasText: "Settled" }).click(); }),
    wait: ".mb-list.settled .mb-settled", vp: "m390", full: false, mockup: ["profile", "cashout"],
  },
  {
    name: "receipt-390",
    setup: onBets(async (p) => { await p.locator(".mb-card .receipt-link").first().click(); }),
    wait: "#receipt", vp: "m390", full: true, mockup: ["placed", "cashout"],
  },
  { name: "cashout-390", setup: celebrate, wait: "#cashout-celebration #celebrate-amount", vp: "m390", full: false, mockup: ["cashout"] },
  { name: "cashout-frame", setup: celebrate, wait: "#cashout-celebration #celebrate-amount", vp: "f-cashout", full: false, match: true, mockup: ["cashout"] },
];
