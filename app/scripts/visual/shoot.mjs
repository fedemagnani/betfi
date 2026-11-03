// Visual review (6.0a, 6.2a-g; v2 targets by 7.0a). Screenshots of the app next to the BetFi C2
// mockup, and one HTML page for the user's sign-off:
//   1. Kit specimens: /dev/kit's `#spec-*` frames (the mockup's pieces rebuilt from the kit with
//      the mockup's content), each shot as an element at 390 px and put next to the matching crop
//      of the mockup screen it comes from (CROPS below, in the mockup's CSS px).
//   2. Screens: the shell and the L6-1/L7-1 step shots (scripts/visual/shots/<step>.mjs) at 390 px
//      and at their mockup's own frame. Every C2 screen is a 390 px wide phone frame scaled to
//      1600 px tall (the profile to 2x): its own frame is 390 × (390 × height / width) px, and
//      shots with `match: true` are scored against it:
//        - the share of pixels whose colour differs by more than pixelmatch's default threshold
//          (YIQ distance, 0.1), over the area both images cover, and the same on 8 × 8 device-px
//          blocks; computed in the browser's canvas, no dependency;
//        - the difference image (changed pixels red over the dimmed mockup).
//   3. The kit page in full at 390 px and at each mockup's frame height (one shot per distinct
//      frame), plus the shell at 1440 px (V-7: the 480 px centred column).
// A note field per shot keeps the accepted differences (prefilled from notes.json, kept in the
// page's localStorage; "Copy notes" gives the JSON for notes.json). Each shot is checked as the
// phone gate checks a screen (scripts/screen_checks.mjs); kit shots also ban the word "odds".
//
// From app/ (out/ is gitignored: the mockup stays private):
//   SCHEDINA_DESIGN_FIXTURES=1 dx build --platform web
//   node scripts/serve.mjs target/dx/schedina-app/debug/web/public 8765 &
//   PLAYWRIGHT_CORE=../node_modules/playwright-core/index.mjs \
//     node scripts/visual/shoot.mjs http://127.0.0.1:8765 scripts/visual/out
// MOCKUP (default .claude/BetFi-C2 at the repo root) holds the eight `<screen>/screen.png`.
// CHROME_PATH overrides the headless Chromium. Times render in Europe/Rome, the mockup's clock.
// L6's desktop step shots (1024/1440 px, three columns) are skipped since V-7 retired that
// layout; STEP_DESKTOP=1 keeps them. Exits 1 if a shot fails its checks or never renders; the
// last line is a one-line summary with the page's path.
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { problems, screenFacts } from "../screen_checks.mjs";

const { chromium } = await import(process.env.PLAYWRIGHT_CORE ?? "playwright-core");
const [base, outArg] = [process.argv[2], process.argv[3] ?? "scripts/visual/out"];
if (!base) {
  console.error("usage: node scripts/visual/shoot.mjs <base-url> [out-dir]");
  process.exit(2);
}
const out = resolve(outArg);
const MOCKUP = process.env.MOCKUP ?? fileURLToPath(new URL("../../../.claude/BetFi-C2", import.meta.url));
const CHROME = process.env.CHROME_PATH
  ?? `${homedir()}/Library/Caches/ms-playwright/chromium_headless_shell-1208/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const WAIT_MS = 15000;

/// The eight C2 screens (reference images), by short name.
export const SCREENS = {
  sportsbook: "betfi_sportsbook_equal_odds",
  drawer: "betfi_sportsbook_bet_drawer_instagram_style",
  event: "betfi_event_detail_market_volume",
  profile: "betfi_tipster_profile_one_tap_copy",
  copiers: "betfi_tipster_copiers_drawer_comments_style",
  copy: "betfi_copy_trade_schedina_decision",
  placed: "betfi_bet_placement_celebration",
  cashout: "betfi_cash_out_celebration",
};
const screenPng = (s) => join(MOCKUP, SCREENS[s], "screen.png");

/// L6 mockup names the step files still use, mapped onto the C2 screens (desktop ones: none).
const ALIAS = {
  "mobile/01-home.png": "sportsbook", "mobile/02-event.png": "event", "mobile/03-slip.png": "drawer",
  "mobile/04-my-bets.png": "cashout", "mobile/05-copy-tipster.png": "copy",
};
const screenOf = (m) => (SCREENS[m] ? m : ALIAS[m] ?? null);

/// Specimen → [screen, crop {x, y, w, h} in the mockup's CSS px (390 wide)].
export const CROPS = {
  "spec-appbar": ["sportsbook", { x: 0, y: 0, w: 390, h: 92 }],
  "spec-tabs": ["sportsbook", { x: 0, y: 116, w: 390, h: 46 }],
  "spec-hero": ["sportsbook", { x: 0, y: 166, w: 390, h: 172 }],
  "spec-upcoming": ["sportsbook", { x: 0, y: 344, w: 390, h: 444 }],
  "spec-pill": ["sportsbook", { x: 0, y: 824, w: 390, h: 58 }],
  "spec-nav": ["sportsbook", { x: 0, y: 892, w: 390, h: 64 }],
  "spec-drawer": ["drawer", { x: 0, y: 160, w: 390, h: 788 }],
  "spec-copy": ["copy", { x: 0, y: 684, w: 390, h: 572 }],
  "spec-legs": ["copy", { x: 0, y: 322, w: 390, h: 354 }],
  "spec-celebration": ["placed", { x: 0, y: 66, w: 390, h: 926 }],
  "spec-cashout": ["cashout", { x: 0, y: 118, w: 390, h: 444 }],
  "spec-profile": ["profile", { x: 0, y: 96, w: 390, h: 580 }],
  "spec-copiers": ["copiers", { x: 0, y: 232, w: 390, h: 652 }],
  "spec-event": ["event", { x: 0, y: 252, w: 390, h: 284 }],
};

const pngSize = async (file) => {
  const b = await readFile(file);
  return { w: b.readUInt32BE(16), h: b.readUInt32BE(20) };
};
/// Each screen's frame in CSS px: 390 wide, its height by the PNG's aspect.
const FRAMES = {};
for (const s of Object.keys(SCREENS)) {
  if (existsSync(screenPng(s))) {
    const { w, h } = await pngSize(screenPng(s));
    FRAMES[s] = { width: 390, height: Math.round((390 * h) / w), scale: w / 390 };
  }
}

/// Viewports: the 390 × 844 phone, each screen's frame, and a wide desktop.
export const VIEWPORTS = {
  m390: { width: 390, height: 844, label: "390 × 844" },
  d1440: { width: 1440, height: 915, label: "1440 wide" },
  d1024: { width: 1024, height: 1611, label: "1024 wide" },
  ...Object.fromEntries(Object.entries(FRAMES).map(([s, f]) => [`f-${s}`, { width: f.width, height: f.height, label: `390 × ${f.height} (${s} frame)` }])),
};

/// True on a design build: the shell's root carries `data-fixtures="design"` (6.2a-p).
export const DESIGN_FLAG = '[data-fixtures="design"]';
export const isDesign = async (page) => (await page.locator(DESIGN_FLAG).count()) > 0;

// Client-side navigation, so the slip stays as it is.
const go = (page, path) => page.evaluate((p) => {
  history.pushState(null, "", p);
  dispatchEvent(new PopStateEvent("popstate"));
}, path);

// Home with three picks (the slip pill then shows), then `path` in the same session. A design
// build keeps the mockup's seeded slip instead: no picks are added (they would toggle it).
const withPicks = (path) => async (page) => {
  await page.goto(base + "/");
  await page.waitForSelector(".home-game .pick");
  if (await isDesign(page)) {
    await page.waitForSelector("#slip-bar", { state: "attached", timeout: WAIT_MS }).catch(() => {});
  } else {
    const picks = page.locator(".pick:not([disabled])");
    for (const i of [1, 2, 4]) await picks.nth(i).click();
    await page.waitForSelector("#slip-bar", { state: "attached" });
  }
  if (path !== "/") await go(page, path);
};

// The steps' screen shots (L7-1 rewrites each file for its screen).
const STEP_FILES = ["home", "event", "slip", "my-bets", "tipsters", "account", "copy"];
const stepShots = [];
for (const f of STEP_FILES) {
  const file = new URL(`./shots/${f}.mjs`, import.meta.url);
  if (existsSync(file)) {
    const mod = await import(file.href);
    stepShots.push(...(mod.shots?.({ withPicks, isDesign, go }) ?? []));
  }
}
const keepDesktop = process.env.STEP_DESKTOP === "1";
const frameShots = [...new Set(Object.keys(FRAMES).map((s) => FRAMES[s].height))].map((h) => {
  const s = Object.keys(FRAMES).find((k) => FRAMES[k].height === h);
  return { name: `kit-frame-${h}`, path: "/dev/kit", wait: "#kit-gallery", vp: `f-${s}`, full: true, gallery: true, mockup: [s] };
});
export const SHOTS = [
  ...stepShots
    .filter((s) => keepDesktop || !/^d/.test(s.vp))
    .map((s) => ({ ...s, mockup: s.mockup.map(screenOf).filter(Boolean) })),
  { name: "shell-390", setup: withPicks("/dev/shell"), wait: "#kit-shell", vp: "m390", full: false, mockup: ["sportsbook"] },
  { name: "shell-sportsbook-frame", setup: withPicks("/"), wait: "#slip-bar", vp: "f-sportsbook", full: false, match: true, mockup: ["sportsbook"] },
  { name: "shell-1440", setup: withPicks("/dev/shell"), wait: "#kit-shell", vp: "d1440", full: false, mockup: ["sportsbook"] },
  { name: "kit-390", path: "/dev/kit", wait: "#kit-gallery", vp: "m390", full: true, gallery: true, mockup: Object.keys(SCREENS) },
  ...frameShots,
];

await mkdir(join(out, "mockup"), { recursive: true });
await mkdir(join(out, "diff"), { recursive: true });
await mkdir(join(out, "spec"), { recursive: true });
const browser = await chromium.launch({ executablePath: CHROME });
const newCtx = (vp) => browser.newContext({
  viewport: { width: vp.width, height: vp.height }, deviceScaleFactor: 2, colorScheme: "dark",
  timezoneId: "Europe/Rome", serviceWorkers: "block",
  // At rest: no confetti or count-up mid-flight in a shot (the celebrations' motion, 31 Oct).
  reducedMotion: "reduce",
});
const results = [];
let failed = false;
let designBuild = null;
for (const shot of SHOTS) {
  const vp = VIEWPORTS[shot.vp];
  const ctx = await newCtx(vp);
  ctx.setDefaultTimeout(WAIT_MS);
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  // Nothing but the app's own origin: fonts and data are bundled (no CDN calls).
  const foreign = [];
  page.on("request", (r) => { if (!r.url().startsWith(base) && !r.url().startsWith("data:")) foreign.push(r.url()); });
  const file = `${shot.name}.png`;
  let m = { scrollWidth: 0, fonts: [] };
  let bad = [];
  try {
    if (shot.setup) await shot.setup(page); else await page.goto(base + shot.path);
    await page.waitForSelector(shot.wait);
    designBuild ??= await isDesign(page);
    const f = await screenFacts(page);
    bad = problems(f, { toned: !shot.gallery, noOdds: !!shot.gallery });
    m = { scrollWidth: f.scrollWidth, fonts: await page.evaluate(() => [...document.fonts].filter((x) => x.status === "loaded").map((x) => `${x.family} ${x.weight}`)) };
    await page.mouse.move(0, 0);
    await page.screenshot({ path: join(out, file), fullPage: shot.full });
  } catch (e) {
    bad = [`never rendered: ${String(e).split("\n")[0].slice(0, 200)}`];
    await page.screenshot({ path: join(out, file), fullPage: shot.full }).catch(() => {});
  }
  const ok = bad.length === 0 && errors.length === 0 && foreign.length === 0;
  if (!ok) failed = true;
  results.push({ ...shot, setup: undefined, file, viewport: vp.label, ...m, problems: bad, errors, foreign, ok });
  await ctx.close();
}

// Kit specimens: each `#spec-*` as an element at 390 px, the fixed chrome hidden.
const specs = [];
{
  const ctx = await newCtx(VIEWPORTS.m390);
  ctx.setDefaultTimeout(WAIT_MS);
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  try {
    await page.goto(base + "/dev/kit");
    await page.waitForSelector("#kit-gallery");
    await page.addStyleTag({ content: ".app-bottom, .app-top { display: none !important; }" });
    await page.evaluate(() => document.fonts.ready);
    for (const id of Object.keys(CROPS)) {
      const el = page.locator(`#${id}`);
      await el.scrollIntoViewIfNeeded();
      await el.screenshot({ path: join(out, "spec", `${id}.png`) });
      specs.push({ id, file: `spec/${id}.png` });
    }
  } catch (e) {
    failed = true;
    specs.push({ id: "error", error: String(e).split("\n")[0] });
  }
  if (errors.length) { failed = true; specs.push({ id: "error", error: errors.join("; ") }); }
  await ctx.close();
}

// Mockup PNGs next to the shots, and the crops (cut in the browser's canvas).
const dataUrl = async (p) => `data:image/png;base64,${(await readFile(p)).toString("base64")}`;
const mockFile = (s) => `mockup/${s}.png`;
const cropper = await (await browser.newContext()).newPage();
for (const s of Object.keys(FRAMES)) await writeFile(join(out, mockFile(s)), await readFile(screenPng(s)));
for (const sp of specs.filter((x) => x.file)) {
  const [s, r] = CROPS[sp.id];
  if (!FRAMES[s]) continue;
  const k = FRAMES[s].scale;
  const png = await cropper.evaluate(async ([url, r, k]) => {
    const img = await new Promise((ok, no) => { const i = new Image(); i.onload = () => ok(i); i.onerror = no; i.src = url; });
    const w = Math.round(r.w * k); const h = Math.min(Math.round(r.h * k), img.naturalHeight - Math.round(r.y * k));
    const c = new OffscreenCanvas(w, h);
    c.getContext("2d").drawImage(img, Math.round(r.x * k), Math.round(r.y * k), w, h, 0, 0, w, h);
    const bytes = new Uint8Array(await (await c.convertToBlob({ type: "image/png" })).arrayBuffer());
    let bin = "";
    for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return btoa(bin);
  }, [await dataUrl(screenPng(s)), r, k]);
  sp.crop = `spec/${sp.id}-mockup.png`;
  sp.screen = s;
  await writeFile(join(out, sp.crop), Buffer.from(png, "base64"));
}

// Pixel differences of the scored screen shots against their mockup scaled to the shot's width.
const differ = cropper;
for (const r of results) {
  const ref = r.match && r.mockup[0];
  if (!ref || !FRAMES[ref] || !existsSync(join(out, r.file))) continue;
  const d = await differ.evaluate(async ([appUrl, mockUrl]) => {
    const load = (src) => new Promise((ok, no) => { const i = new Image(); i.onload = () => ok(i); i.onerror = no; i.src = src; });
    const [a, b] = await Promise.all([load(appUrl), load(mockUrl)]);
    const w = a.naturalWidth;
    const bh = Math.round(b.naturalHeight * (w / b.naturalWidth));
    const h = Math.min(a.naturalHeight, bh);
    const pixels = (img, sw, sh) => {
      const c = new OffscreenCanvas(w, h).getContext("2d", { willReadFrequently: true });
      c.drawImage(img, 0, 0, sw, sh);
      return c.getImageData(0, 0, w, h).data;
    };
    const A = pixels(a, a.naturalWidth, a.naturalHeight);
    const B = pixels(b, w, bh);
    const yiq = (r, g, bl) => [r * 0.29889531 + g * 0.58662247 + bl * 0.11448223,
      r * 0.59597799 - g * 0.27417610 - bl * 0.32180189, r * 0.21147017 - g * 0.52261711 + bl * 0.31114694];
    const MAX = 35215 * 0.1 * 0.1;
    const delta = (p, q) => { const [y1, i1, q1] = yiq(...p); const [y2, i2, q2] = yiq(...q); return 0.5053 * (y1 - y2) ** 2 + 0.299 * (i1 - i2) ** 2 + 0.1957 * (q1 - q2) ** 2; };
    const diff = new ImageData(w, h);
    let bad = 0;
    for (let k = 0; k < w * h * 4; k += 4) {
      const off = delta([A[k], A[k + 1], A[k + 2]], [B[k], B[k + 1], B[k + 2]]) > MAX;
      if (off) bad++;
      const dim = (B[k] * 0.3 + B[k + 1] * 0.59 + B[k + 2] * 0.11) * 0.35;
      diff.data[k] = off ? 255 : dim; diff.data[k + 1] = off ? 40 : dim; diff.data[k + 2] = off ? 60 : dim; diff.data[k + 3] = 255;
    }
    const S = 8;
    let blocks = 0;
    let badBlocks = 0;
    for (let by = 0; by + S <= h; by += S) {
      for (let bx = 0; bx + S <= w; bx += S) {
        const mean = (D) => {
          let r = 0; let g = 0; let bl = 0;
          for (let y = by; y < by + S; y++) for (let x = bx; x < bx + S; x++) { const k = (y * w + x) * 4; r += D[k]; g += D[k + 1]; bl += D[k + 2]; }
          return [r / S / S, g / S / S, bl / S / S];
        };
        blocks++;
        if (delta(mean(A), mean(B)) > MAX) badBlocks++;
      }
    }
    const c = new OffscreenCanvas(w, h);
    c.getContext("2d").putImageData(diff, 0, 0);
    const bytes = new Uint8Array(await (await c.convertToBlob({ type: "image/png" })).arrayBuffer());
    let bin = "";
    for (let k = 0; k < bytes.length; k += 0x8000) bin += String.fromCharCode(...bytes.subarray(k, k + 0x8000));
    return { w, h, px: bad / (w * h), blocks: badBlocks / Math.max(1, blocks), png: btoa(bin) };
  }, [await dataUrl(join(out, r.file)), await dataUrl(screenPng(ref))]);
  await writeFile(join(out, "diff", r.file), Buffer.from(d.png, "base64"));
  r.score = { ref, px: d.px, blocks: d.blocks, area: `${Math.round(d.w / 2)} × ${Math.round(d.h / 2)}`, diff: `diff/${r.file}` };
}
await browser.close();

// The review page.
const notesFile = new URL("./notes.json", import.meta.url);
const notes = existsSync(notesFile) ? JSON.parse(await readFile(notesFile, "utf8")) : {};
// Keys that are not one shot's note (`_matches`: what each parity screen matches) ride along
// when the page copies the notes back out.
const extraNotes = Object.fromEntries(Object.entries(notes).filter(([, v]) => typeof v !== "string"));
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const pct = (x) => `${(x * 100).toFixed(1)}%`;
const scored = results.filter((r) => r.score);
const note = (name) => `<label class="note">Accepted differences<textarea data-name="${esc(name)}" rows="2" placeholder="What differs from the mockup and why it is accepted (decision ids)">${esc(notes[name] ?? "")}</textarea></label>`;
const specRows = specs.filter((s) => s.file).map((s) => `
<section id="${esc(s.id)}">
  <h2>${esc(s.id)} <small>kit specimen at 390 px · mockup ${esc(s.screen ?? "")} crop</small></h2>
  ${note(s.id)}
  <div class="pair">
    <figure><figcaption>Mockup (${esc(SCREENS[s.screen] ?? "")})</figcaption><img loading="lazy" class="half" src="${esc(s.crop ?? "")}" alt="mockup crop ${esc(s.id)}"></figure>
    <figure><figcaption>Kit</figcaption><img loading="lazy" class="half" src="${esc(s.file)}" alt="kit ${esc(s.id)}"></figure>
  </div>
</section>`).join("\n");
const summaryRows = results.map((r) => `<tr><td><a href="#${esc(r.name)}">${esc(r.name)}</a></td><td>${esc(r.viewport)}</td>`
  + `<td>${r.score ? esc(r.score.ref) : `<span class="muted">reference only</span>`}</td>`
  + `<td class="n">${r.score ? pct(r.score.px) : ""}</td><td class="n">${r.score ? pct(r.score.blocks) : ""}</td>`
  + `<td>${r.ok ? "ok" : "<b>CHECK FAILED</b>"}</td><td class="note-cell" data-for="${esc(r.name)}"></td></tr>`).join("\n");
const sections = results.map((r) => `
<section id="${esc(r.name)}">
  <h2>${esc(r.name)} <small>${esc(r.viewport)} · ${r.ok ? "dark, no horizontal scroll, values toned, tabular Plus Jakarta Sans" : `<b>CHECK FAILED</b>: ${esc([...r.problems, ...r.errors, ...r.foreign].join("; "))}`}</small></h2>
  ${r.score ? `<p class="score">Pixel difference against ${esc(r.score.ref)}: <b>${pct(r.score.px)}</b> of pixels, <b>${pct(r.score.blocks)}</b> of 8 × 8 px blocks, over ${esc(r.score.area)} px.</p>` : `<p class="score muted">Not scored: the mockup is a style reference for this shot.</p>`}
  ${note(r.name)}
  <div class="pair">
    <figure><figcaption>App</figcaption><img loading="lazy" class="half" src="${esc(r.file)}" alt="${esc(r.name)}"></figure>
    ${r.mockup.slice(0, r.gallery ? 8 : 2).map((m) => `<figure><figcaption>Mockup ${esc(m)}</figcaption><img loading="lazy" class="half" src="${esc(mockFile(m))}" alt="mockup ${esc(m)}"></figure>`).join("\n    ")}
    ${r.score ? `<figure><figcaption>Difference (red: changed)</figcaption><img loading="lazy" class="half" src="${esc(r.score.diff)}" alt="difference ${esc(r.name)}"></figure>` : ""}
  </div>
</section>`).join("\n");
await writeFile(join(out, "index.html"), `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>BetFi side by side</title>
<style>
:root { color-scheme: dark; --bg: #05060b; --text: #e3e1e9; --muted: #978da1; --line: #292a2f; --accent: #56ffa8; }
body { margin: 0; padding: 24px; background: var(--bg); color: var(--text); font: 14px system-ui, sans-serif; }
h1 { margin: 0 0 4px; } p { color: var(--muted); margin: 0 0 12px; } .muted { color: var(--muted); }
table { border-collapse: collapse; margin: 0 0 32px; font-size: 13px; }
th, td { border-bottom: 1px solid var(--line); padding: 4px 10px; text-align: left; vertical-align: top; }
td.n { text-align: right; font-variant-numeric: tabular-nums; } td.note-cell { color: var(--muted); max-width: 420px; }
a { color: var(--accent); }
section { margin-bottom: 40px; }
h2 { font-size: 16px; margin-bottom: 6px; } h2 small { color: var(--muted); font-weight: 400; }
.score { color: var(--text); } .score b { color: var(--accent); }
.note { display: block; color: var(--muted); font-size: 12px; margin: 0 0 12px; max-width: 900px; }
.note textarea { display: block; width: 100%; margin-top: 4px; background: #121318; color: var(--text); border: 1px solid var(--line); border-radius: 6px; padding: 6px 8px; font: inherit; font-size: 13px; }
.pair { display: flex; gap: 16px; align-items: flex-start; overflow-x: auto; }
figure { margin: 0; flex: none; }
figcaption { color: var(--muted); font-size: 12px; margin-bottom: 6px; }
img { display: block; border: 1px solid var(--line); }
img.half { width: 390px; height: auto; }
.bar { display: flex; gap: 8px; align-items: center; margin: 0 0 16px; }
button { background: #1a1b21; color: var(--text); border: 1px solid var(--line); border-radius: 16px; padding: 6px 14px; font: inherit; cursor: pointer; }
#notes-json { display: none; width: 100%; max-width: 900px; height: 160px; background: #121318; color: var(--text); border: 1px solid var(--line); font: 12px ui-monospace, monospace; }
</style></head><body>
<h1>BetFi: app next to the mockup</h1>
<p>${specs.filter((s) => s.file).length} kit specimens next to their mockup crops, then ${results.length} shots at 2x from ${esc(base)} (${designBuild ? "design build: the mockup's content" : "demo build: example data"}); mockup: BetFi C2 (eight screens, each a 390 px frame). Generated by app/scripts/visual/shoot.mjs. Scores are the share of pixels (and of 8 × 8 px blocks) whose colour differs from the mockup; content that differs by decision (chances for scores, guidelines over screens) counts too, so read them next to the notes.</p>
<div class="bar"><button id="copy-notes" type="button">Copy notes as JSON</button><span class="muted" id="notes-state">Notes are kept in this browser; paste the JSON into app/scripts/visual/notes.json to keep them.</span></div>
<textarea id="notes-json" readonly></textarea>
<h1>Kit specimens</h1>
${specRows}
<h1>Shots</h1>
<table><thead><tr><th>Shot</th><th>Viewport</th><th>Mockup</th><th>Pixels off</th><th>Blocks off</th><th>Checks</th><th>Note</th></tr></thead><tbody>
${summaryRows}
</tbody></table>
${sections}
<script>
(() => {
  const KEY = "betfi-review-notes-v2";
  let saved = {};
  try { saved = JSON.parse(localStorage.getItem(KEY) || "{}"); } catch (e) { saved = {}; }
  const areas = [...document.querySelectorAll("textarea[data-name]")];
  const cell = (name) => document.querySelector('.note-cell[data-for="' + CSS.escape(name) + '"]');
  const show = (a) => { const c = cell(a.dataset.name); if (c) c.textContent = a.value; };
  for (const a of areas) {
    if (typeof saved[a.dataset.name] === "string") a.value = saved[a.dataset.name];
    show(a);
    a.addEventListener("input", () => {
      saved[a.dataset.name] = a.value;
      show(a);
      try { localStorage.setItem(KEY, JSON.stringify(saved)); } catch (e) { /* private window */ }
    });
  }
  document.getElementById("copy-notes").addEventListener("click", async () => {
    const all = { ...Object.fromEntries(areas.filter((a) => a.value.trim()).map((a) => [a.dataset.name, a.value.trim()])), ...${JSON.stringify(extraNotes).replace(/</g, "\\u003c")} };
    const text = JSON.stringify(all, null, 2) + "\\n";
    const box = document.getElementById("notes-json");
    box.value = text;
    box.style.display = "block";
    box.select();
    let copied = false;
    try { await navigator.clipboard.writeText(text); copied = true; } catch (e) { copied = false; }
    document.getElementById("notes-state").textContent = copied ? "Copied. Paste into app/scripts/visual/notes.json." : "Select the text below and copy it.";
  });
})();
</script>
</body></html>
`);

// The parity review (7.2a-g): the eight mockup screens, each next to the app at the mockup's own
// frame and their difference image, with the score, what matches and the accepted differences
// (notes.json: the shot's note, its decision ids, and `_matches`), as parity.html and parity.md.
const PARITY = [
  ["sportsbook", "home-frame", "Sportsbook"], ["drawer", "drawer-frame", "Bet drawer"], ["event", "event-frame", "Event detail"],
  ["profile", "profile-frame", "Tipster profile"], ["copiers", "copiers-frame", "Copiers drawer"],
  ["copy", "copy-confirm-frame", "Copy confirm"], ["placed", "celebration-frame", "Bet placement celebration"],
  ["cashout", "cashout-frame", "Cash-out celebration"],
];
const matches = notes._matches ?? {};
// A note's sentences (each ends with its decision ids in brackets), and every id it cites.
const sentences = (t) => (t ?? "").split(/(?<=[.)])\s+(?=[A-Z'"‘“€0-9≈])/).map((x) => x.trim()).filter(Boolean);
const ids = (t) => [...new Set((t ?? "").match(/\b(?:D-\d{3}|V-\d)\b/g) ?? [])]
  .sort((a, b) => a[0].localeCompare(b[0]) || Number(a.slice(2)) - Number(b.slice(2)));
const parity = PARITY.map(([screen, shot, title]) => {
  const r = results.find((x) => x.name === shot);
  return { screen, shot, title, mockup: SCREENS[screen], frame: FRAMES[screen], r, matches: matches[shot] ?? "", accepted: notes[shot] ?? "" };
});
const scoredParity = parity.filter((p) => p.r?.score);
const meanOf = (k) => (scoredParity.length ? pct(scoredParity.reduce((a, p) => a + p.r.score[k], 0) / scoredParity.length) : "n/a");
const parityRows = parity.map((p) => `<tr><td><a href="#p-${esc(p.screen)}">${esc(p.title)}</a></td><td>${esc(p.mockup)}</td>`
  + `<td>${p.frame ? `390 × ${p.frame.height}` : "no mockup"}</td>`
  + `<td class="n">${p.r?.score ? pct(p.r.score.px) : "–"}</td><td class="n">${p.r?.score ? pct(p.r.score.blocks) : "–"}</td>`
  + `<td>${!p.r ? "<b>not shot</b>" : p.r.ok ? "ok" : "<b>CHECK FAILED</b>"}</td><td>${esc(ids(p.accepted).join(", "))}</td></tr>`).join("\n");
const paritySections = parity.map((p) => `
<section id="p-${esc(p.screen)}">
  <h2>${esc(p.title)} <small>${esc(p.mockup)} · app shot ${esc(p.shot)} at the mockup's frame${p.frame ? ` (390 × ${p.frame.height} px)` : ""}</small></h2>
  ${p.r?.score ? `<p class="score">Pixel difference: <b>${pct(p.r.score.px)}</b> of pixels, <b>${pct(p.r.score.blocks)}</b> of 8 × 8 px blocks, over ${esc(p.r.score.area)} px.</p>` : `<p class="score"><b>Not scored</b>: ${p.r ? esc([...p.r.problems, ...p.r.errors].join("; ") || "no score") : "the shot is missing"}.</p>`}
  <div class="pair">
    <figure><figcaption>Mockup</figcaption><img loading="lazy" class="half" src="${esc(mockFile(p.screen))}" alt="mockup ${esc(p.screen)}"></figure>
    ${p.r ? `<figure><figcaption>App</figcaption><img loading="lazy" class="half" src="${esc(p.r.file)}" alt="app ${esc(p.shot)}"></figure>` : ""}
    ${p.r?.score ? `<figure><figcaption>Difference (red: changed)</figcaption><img loading="lazy" class="half" src="${esc(p.r.score.diff)}" alt="difference ${esc(p.shot)}"></figure>` : ""}
    <div class="text">
      <h3>Matches</h3><p>${esc(p.matches || "(no note)")}</p>
      <h3>Accepted differences</h3>
      <ul>${sentences(p.accepted).map((x) => `<li>${esc(x)}</li>`).join("") || "<li>(no note)</li>"}</ul>
      <p class="muted">Decisions: ${esc(ids(p.accepted).join(", ") || "none cited")}</p>
    </div>
  </div>
</section>`).join("\n");
await writeFile(join(out, "parity.html"), `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>BetFi parity review</title>
<style>
:root { color-scheme: dark; --bg: #05060b; --text: #e3e1e9; --muted: #978da1; --line: #292a2f; --accent: #56ffa8; }
body { margin: 0; padding: 24px; background: var(--bg); color: var(--text); font: 14px system-ui, sans-serif; }
h1 { margin: 0 0 4px; } p { margin: 0 0 12px; } .muted { color: var(--muted); }
table { border-collapse: collapse; margin: 0 0 32px; font-size: 13px; }
th, td { border-bottom: 1px solid var(--line); padding: 4px 10px; text-align: left; vertical-align: top; }
td.n { text-align: right; font-variant-numeric: tabular-nums; }
a { color: var(--accent); }
section { margin-bottom: 48px; }
h2 { font-size: 16px; margin-bottom: 6px; } h2 small { color: var(--muted); font-weight: 400; }
h3 { font-size: 13px; margin: 0 0 6px; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
.score b { color: var(--accent); }
.pair { display: flex; gap: 16px; align-items: flex-start; overflow-x: auto; }
figure { margin: 0; flex: none; }
figcaption { color: var(--muted); font-size: 12px; margin-bottom: 6px; }
img { display: block; border: 1px solid var(--line); }
img.half { width: 300px; height: auto; }
.text { flex: 1 0 320px; max-width: 520px; font-size: 13px; line-height: 1.45; }
.text ul { margin: 0 0 12px; padding-left: 18px; } .text li { margin-bottom: 4px; }
</style></head><body>
<h1>BetFi parity: the eight mockup screens</h1>
<p class="muted">Each BetFi C2 screen next to the app (design build, ${esc(base)}) at the mockup's own frame (390 px wide, 2x), and their difference image. Scores are informative (D-414): content that differs by decision counts too. Accepted differences come from app/scripts/visual/notes.json and follow plan-ui2's precedence (guidelines over screens) and truth tables. Mean over ${scoredParity.length} screens: ${meanOf("px")} pixels, ${meanOf("blocks")} blocks off. Generated by app/scripts/visual/shoot.mjs; every other shot is on <a href="index.html">the full review page</a>.</p>
<table><thead><tr><th>Screen</th><th>Mockup</th><th>Frame</th><th>Pixels off</th><th>Blocks off</th><th>Checks</th><th>Decisions</th></tr></thead><tbody>
${parityRows}
</tbody></table>
${paritySections}
</body></html>
`);
const md = [
  "# BetFi parity: the eight mockup screens",
  "",
  `Generated by \`app/scripts/visual/shoot.mjs\` (\`just visual\`) from the design build; the side-by-side page is \`parity.html\` beside this file. Each app shot is taken at the mockup's own frame (390 px wide). Scores are the share of pixels, and of 8 × 8 px blocks, whose colour differs from the mockup (informative, D-414: differences by decision count too). Accepted differences are \`app/scripts/visual/notes.json\`'s, under plan-ui2's precedence (guidelines over screens) and truth tables.`,
  "",
  "| Screen | App shot | Frame | Pixels off | Blocks off | Checks | Decisions |",
  "| --- | --- | --- | ---: | ---: | --- | --- |",
  ...parity.map((p) => `| ${p.title} | \`${p.shot}\` | ${p.frame ? `390 × ${p.frame.height}` : "–"} | ${p.r?.score ? pct(p.r.score.px) : "–"} | ${p.r?.score ? pct(p.r.score.blocks) : "–"} | ${!p.r ? "not shot" : p.r.ok ? "ok" : "CHECK FAILED"} | ${ids(p.accepted).join(", ")} |`),
  `| **Mean** | | | **${meanOf("px")}** | **${meanOf("blocks")}** | | |`,
  "",
  ...parity.flatMap((p) => [
    `## ${p.title}`,
    "",
    `Mockup \`${p.mockup}\`; app \`${p.shot}\`${p.r?.score ? `: ${pct(p.r.score.px)} of pixels, ${pct(p.r.score.blocks)} of blocks off over ${p.r.score.area} px` : ": not scored"}.`,
    "",
    `**Matches.** ${p.matches || "(no note)"}`,
    "",
    "**Accepted differences.**",
    "",
    ...(sentences(p.accepted).map((x) => `- ${x}`).concat(sentences(p.accepted).length ? [] : ["- (no note)"])),
    "",
    `Decisions: ${ids(p.accepted).join(", ") || "none cited"}.`,
    "",
  ]),
].join("\n");
await writeFile(join(out, "parity.md"), md);

console.log(JSON.stringify({
  specimens: specs.map((s) => s.error ? { id: s.id, error: s.error } : s.id),
  shots: results.map(({ name, viewport, scrollWidth, fonts, problems: p, errors, foreign, ok, score }) =>
    ({ name, viewport, scrollWidth, fonts: [...new Set(fonts)], problems: p, errors, foreign, ok, ...(score ? { px: +(score.px * 100).toFixed(2), blocks: +(score.blocks * 100).toFixed(2) } : {}) })),
}, null, 1));
const mean = (k) => scored.length ? pct(scored.reduce((a, r) => a + r.score[k], 0) / scored.length) : "n/a";
console.log(`VISUAL SHOTS ${failed ? "FAILED" : "OK"}: ${specs.filter((s) => s.file).length} kit specimens next to their mockup crops; `
  + `${results.filter((r) => r.ok).length}/${results.length} shots on the ${designBuild ? "design" : "demo"} build `
  + `(dark, no horizontal scroll, values toned, tabular Plus Jakarta Sans, no "odds" on the kit, no page errors, no foreign requests); `
  + `${scored.length} scored against their mockup, mean ${mean("px")} pixels / ${mean("blocks")} blocks off; `
  + `parity ${scoredParity.length}/${parity.length} mockup screens at their frames, mean ${meanOf("px")} pixels off (${join(out, "parity.html")}, parity.md); `
  + `side by side: ${join(out, "index.html")}`);
process.exit(failed || scoredParity.length < parity.length ? 1 : 0);
