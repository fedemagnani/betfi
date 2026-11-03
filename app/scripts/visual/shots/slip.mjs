// L7-1 visual shots for the bet drawer and the placement celebration (7.1b), loaded by ../shoot.mjs.
// Mockups: BetFi C2 `betfi_sportsbook_bet_drawer_instagram_style` and
// `betfi_bet_placement_celebration` (390 px frames).
// The drawer: a design build seeds the mockup's three picks and €25 (fixtures/design/slip.json);
// the slip pill opens the drawer over the Sportsbook. A plain demo build gets the shooter's three
// picks first. The celebration: a design build's `/confirm` with nothing placed shows the mockup's
// ticket (fixtures/design/ticket.json, slot 284,912,041), shot as it opens; the placed flow (Bet →
// the CTA's escrowed → hedging → minted → `/confirm`) is shot as a reference too.
const drawer = (withPicks, after) => async (page) => {
  await withPicks("/")(page);
  await page.locator("#slip-bar").click();
  // The design slip seeds a price move (D-492): accept it so Bet arms (7.2a-p, D-541).
  await page.waitForSelector("#bet-drawer #price-moved .k-moved-accept, #bet-drawer #gioca:not([disabled])");
  if (await page.locator("#price-moved").count()) await page.locator("#price-moved .k-moved-accept").click();
  await page.waitForSelector("#bet-drawer #gioca:not([disabled])");
  if (after) await after(page);
  await page.mouse.move(0, 0);
};

const celebration = async (page) => {
  await page.goto(process.argv[2] + "/confirm");
  await page.waitForSelector("#celebration #celebrate-slip");
  await page.evaluate(() => document.fonts.ready);
};

// Bet, then wait for the drawer's progress steps to reach `stage` (a word of a step).
const bet = (stage) => async (page) => {
  if (await page.locator("#price-moved").count()) await page.locator("#price-moved .k-moved-accept").click();
  await page.waitForSelector("#gioca:not([disabled])");
  await page.locator("#gioca").click();
  if (stage) await page.waitForSelector(`#bet-drawer .slip-progress >> text=${stage}`);
};

export const shots = ({ withPicks }) => [
  { name: "drawer-390", setup: drawer(withPicks), wait: "#bet-drawer", vp: "m390", full: false, match: true, mockup: ["drawer"] },
  { name: "drawer-frame", setup: drawer(withPicks), wait: "#bet-drawer", vp: "f-drawer", full: false, match: true, mockup: ["drawer"] },
  { name: "drawer-390-hedging", setup: drawer(withPicks, bet("Hedging")), wait: "#bet-drawer .slip-progress", vp: "m390", full: false, mockup: ["drawer"] },
  { name: "celebration-390", setup: celebration, wait: "#celebration", vp: "m390", full: false, match: true, mockup: ["placed"] },
  { name: "celebration-frame", setup: celebration, wait: "#celebration", vp: "f-placed", full: false, match: true, mockup: ["placed"] },
  { name: "celebration-390-placed", setup: drawer(withPicks, async (page) => {
    await bet()(page);
    await page.waitForURL("**/confirm", { timeout: 15000 });
    await page.waitForSelector("#celebration #celebrate-slip");
  }), wait: "#celebration", vp: "m390", full: true, mockup: ["placed"] },
];
