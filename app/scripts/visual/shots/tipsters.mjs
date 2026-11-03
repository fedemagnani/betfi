// Visual shots for the tipsters screens (7.1d; L9 since 1 Nov): the tipster's page next to
// betfi_tipster_profile_one_tap_copy and its copiers drawer next to
// betfi_tipster_copiers_drawer_comments_style, each at 390 × 844 and scored at the mockup's own
// frame; then the profile in full (the track record below the fold) and the Tipsters list (no
// mockup: shown next to the profile for its style). A design build's tipster is @lorenzo_hoops
// (fixtures/design/tipster.json: Master Treble, 1,284 copiers); a plain demo build takes its
// example tipster (fixtures/desk/tipster.json), chosen by the build itself.
import { readFileSync } from "node:fs";

const tipster = (dir) => JSON.parse(readFileSync(new URL(`../../../fixtures/${dir}/tipster.json`, import.meta.url), "utf8"));

export const shots = ({ isDesign, go }) => {
  const base = process.argv[2];
  // Home first (to tell the builds apart), then the page in the same session.
  const open = (path, after) => async (page) => {
    await page.goto(base + "/");
    await page.waitForSelector("#main");
    const t = tipster((await isDesign(page)) ? "design" : "desk");
    await go(page, path(t.tipster.wallet));
    if (after) await after(page);
  };
  const profile = (w) => `/tipsters/${w}`;
  // L9: the copiers sheet opens from a parlay's "Copied by (n)": the first open parlay, then
  // the sheet's rows.
  const drawer = async (page) => {
    await page.waitForSelector("#open-tips .tp-parlay");
    await page.locator("#open-tips .tp-parlay").first().click();
    await page.waitForSelector("#parlay-copiers");
    await page.locator("#parlay-copiers").click();
    await page.waitForSelector("#copiers-sheet .k-copier, #copiers-empty");
    // The sheet rises in 0.25 s; shoot it settled.
    await page.waitForTimeout(400);
  };
  return [
    { name: "profile-frame", setup: open(profile), wait: "#open-tips .tp-parlay", vp: "f-profile", full: false, match: true, mockup: ["profile"] },
    { name: "profile-390", setup: open(profile), wait: "#open-tips .tp-parlay", vp: "m390", full: false, mockup: ["profile"] },
    { name: "copiers-frame", setup: open(profile, drawer), wait: "#copiers-sheet", vp: "f-copiers", full: false, match: true, mockup: ["copiers"] },
    { name: "copiers-390", setup: open(profile, drawer), wait: "#copiers-sheet", vp: "m390", full: false, mockup: ["copiers"] },
    { name: "tipsters-390", setup: open(() => "/tipsters"), wait: "#board .tp-row", vp: "m390", full: true, mockup: ["profile"] },
  ];
};