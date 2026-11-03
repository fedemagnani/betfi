// Visual shots for a tipster's parlay (L9, replacing 7.1e's Copy confirm), loaded by
// ../shoot.mjs: `/copy/<Master Treble>` (its games locked, "Copied by", Copy Schedina) next to
// betfi_copy_trade_schedina_decision at 390 × 844 (full page) and at the mockup's own frame
// (scored), the copy drawer, then the placement celebration on /confirm after Copy (demo flow,
// no wallet) next to betfi_bet_placement_celebration. A design build's tip is @lorenzo_hoops's
// "Master Treble" (fixtures/design/tipster.json `open[0]`); a plain demo build takes its own
// example tip (fixtures/desk/tipster.json), chosen by the build itself.
import { readFileSync } from "node:fs";

const tipster = (dir) => JSON.parse(readFileSync(new URL(`../../../fixtures/${dir}/tipster.json`, import.meta.url), "utf8"));
const READY = "#tip-copy, #copy-missing";

export const shots = ({ isDesign, go }) => {
  const base = process.argv[2];
  // A page load on the shell, then the tip's copy page in-app (the build tells which tip).
  const open = (after) => async (page) => {
    await page.goto(base + "/tipsters");
    await page.waitForSelector("main.app-main > *");
    const t = tipster((await isDesign(page)) ? "design" : "desk");
    await go(page, `/copy/${t.open[0].ticket}`);
    await page.waitForSelector(READY);
    if (after) await after(page);
    await page.evaluate(() => window.scrollTo(0, 0));
  };
  // Copy Schedina: the drawer in copy mode, priced.
  const drawer = open(async (page) => {
    await page.locator("#tip-copy").click();
    await page.waitForSelector("#bet-drawer #gioca:not([disabled])", { timeout: 15000 });
    await page.waitForTimeout(400);
  });
  // Copy, then the demo stream's escrowed → hedging → minted, then the celebration on /confirm.
  const placed = open(async (page) => {
    await page.locator("#tip-copy").click();
    await page.waitForSelector("#bet-drawer #gioca:not([disabled])", { timeout: 15000 });
    await page.locator("#bet-drawer #gioca").click();
    await page.waitForSelector("#celebration", { timeout: 20000 });
  });
  return [
    { name: "copy-confirm-390", setup: open(), wait: READY, vp: "m390", full: true, mockup: ["copy"] },
    { name: "copy-confirm-frame", setup: open(), wait: READY, vp: "f-copy", full: false, match: true, mockup: ["copy"] },
    { name: "copy-drawer-390", setup: drawer, wait: "#bet-drawer", vp: "m390", full: false, mockup: ["copy"] },
    { name: "copy-placed-390", setup: placed, wait: "#celebration", vp: "m390", full: true, mockup: ["placed"] },
  ];
};
