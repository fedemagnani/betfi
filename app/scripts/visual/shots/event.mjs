// L7-1 visual shots for Event Detail (step 7.1c, BetFi C2 `betfi_event_detail_market_volume`),
// loaded by ../shoot.mjs. A design build seeds the mockup's slip (Panathinaikos, Real Madrid,
// Celtics; fixtures/design/slip.json), so the featured game opens with Panathinaikos pressed, as
// the mockup draws one outcome selected; nothing is tapped (a tap would take a seeded pick out).
// Shots: the featured live game at 390 × 844 and at the mockup's own frame (scored), the soccer
// 1-X-2 game, and an unstarted NBA game with "More markets" opened on Total. On a plain demo
// build (no design games) each shot opens the demo card's first game instead.
import { readFileSync } from "node:fs";
import { byStart } from "../../slate.mjs";

// shoot.mjs passes only helpers; the base URL is its first argument.
const base = process.argv[2];
const FEN_PAO = "/event/el-fen-pao-2026-10-29";
const ARS_BAY = "/event/ucl-ars-bay-2026-10-29";
const BOS_NYK = "/event/nba-bos-nyk-2026-10-29";
const demoFirst = () => {
  const card = JSON.parse(readFileSync(new URL("../../../fixtures/desk/card.json", import.meta.url), "utf8"));
  return "/event/" + byStart(card.games)[0].game_id;
};

// The design build's seeded slip, then `path` in the same session (client-side navigation), with
// `more` ("spread" | "total") opened; the demo build's first game, as it opens.
const open = (helpers, path, more) => async (page) => {
  const { isDesign, go } = helpers;
  await page.goto(base + "/");
  await page.waitForSelector("#main");
  const design = await isDesign(page);
  if (design) await page.waitForSelector("#slip-bar", { state: "attached", timeout: 15000 }).catch(() => {});
  await go(page, design ? path : demoFirst());
  await page.waitForSelector("#event-teams, #event .ev-note-link");
  if (design && more) {
    await page.locator(`#event-tab-${more}`).click();
    await page.waitForSelector(`#event-tab-${more}[aria-pressed="true"]`);
  }
  await page.mouse.move(0, 0);
  await page.evaluate(() => window.scrollTo(0, 0));
};

export const shots = (helpers) => [
  { name: "event-390", setup: open(helpers, FEN_PAO), wait: "#event-sentiment", vp: "m390", full: false, match: true, mockup: ["event"] },
  { name: "event-frame", setup: open(helpers, FEN_PAO), wait: "#event-sentiment", vp: "f-event", full: false, match: true, mockup: ["event"] },
  { name: "event-soccer-390", setup: open(helpers, ARS_BAY), wait: "#event-outcomes", vp: "m390", full: true, mockup: ["event"] },
  { name: "event-more-390", setup: open(helpers, BOS_NYK, "total"), wait: "#event-outcomes", vp: "m390", full: true, mockup: ["event"] },
];
