// L7-1 visual shots for the Sportsbook (7.1a), loaded by ../shoot.mjs. Mockup: BetFi C2
// `betfi_sportsbook_equal_odds` (a 390 px frame).
// A design build opens in the mockup's state: the seeded 3-pick slip (the pill shows), the
// Basketball tab, the live Panathinaikos–Fenerbahçe card, then the upcoming cards; it is shot as
// it opens. A plain demo build gets the shooter's three picks (`withPicks`). The Soccer tab shot
// shows the 1–X–2 card (the mockup draws it under the Basketball tab; here each tab lists its own
// sport, V-2), so it is a reference, not scored.
const sportsbook = (withPicks, sport) => async (page) => {
  await withPicks("/")(page);
  await page.waitForSelector("#home .k-match");
  if (sport) {
    await page.locator(`#sport-${sport}`).click();
    await page.waitForSelector(`#sport-${sport}[aria-pressed="true"]`);
    await page.waitForSelector("#home .k-match");
  }
  await page.mouse.move(0, 0);
  await page.evaluate(() => window.scrollTo(0, 0));
};

export const shots = ({ withPicks }) => [
  { name: "home-390", setup: sportsbook(withPicks), wait: "#home .k-match", vp: "m390", full: false, match: true, mockup: ["sportsbook"] },
  { name: "home-frame", setup: sportsbook(withPicks), wait: "#home .k-match", vp: "f-sportsbook", full: false, match: true, mockup: ["sportsbook"] },
  { name: "home-390-full", setup: sportsbook(withPicks), wait: "#home .k-match", vp: "m390", full: true, mockup: ["sportsbook"] },
  { name: "home-soccer-frame", setup: sportsbook(withPicks, "soccer"), wait: "#sport-soccer[aria-pressed='true']", vp: "f-sportsbook", full: false, mockup: ["sportsbook"] },
];
