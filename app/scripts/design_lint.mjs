// Design lint (L9, design system v3): the stylesheets use the scale in main.css's :root, never
// raw values. Outside :root it refuses
//   - a font size in px (font-size or a `font:` shorthand) instead of var(--fs-*),
//   - a font weight other than 400 / 500 / 600 / 700,
//   - a border radius in px above 2 instead of var(--r-sm|md|lg|full) or 50%,
//   - padding, margin or gap off the 4 px grid (0 1 2 4 8 12 16 20 24 32 40 48 64),
//   - a raw colour (rgba()/rgb()/hex other than #fff and #000) instead of a colour role,
//   - a box-shadow other than var(--shadow-1|2|sheet|cta), var(--focus), none, an inset focus
//     line or a `0 0 0 Npx var(--…)` separation ring.
// A line ending in `/* design-lint: allow */` is a deliberate exception (the celebration's
// confetti colours). Exits 1 with one line per finding.
//
//   node scripts/design_lint.mjs
import { readFileSync, readdirSync } from "node:fs";

const dir = new URL("../assets/", import.meta.url);
const files = ["main.css", ...readdirSync(new URL("screens/", dir)).filter((f) => f.endsWith(".css")).map((f) => `screens/${f}`)];
const GRID = new Set([0, 1, 2, 4, 8, 12, 16, 20, 24, 32, 40, 48, 64]);
const findings = [];

for (const f of files) {
  const lines = readFileSync(new URL(f, dir), "utf8").split("\n");
  let inRoot = false;
  let inComment = false;
  lines.forEach((raw, i) => {
    if (/^:root\s*\{/.test(raw)) inRoot = true;
    if (inRoot) {
      if (/^\}/.test(raw)) inRoot = false;
      return;
    }
    if (/design-lint: allow/.test(raw)) return;
    // Comments out.
    let line = raw;
    if (inComment) {
      const end = line.indexOf("*/");
      if (end < 0) return;
      line = line.slice(end + 2);
      inComment = false;
    }
    line = line.replace(/\/\*.*?\*\//g, "");
    const open = line.indexOf("/*");
    if (open >= 0) { inComment = true; line = line.slice(0, open); }
    const at = `${f}:${i + 1}`;
    for (const m of line.matchAll(/(?<![\w-])([a-z-]+)\s*:\s*([^;{}]+)/g)) {
      const [, prop, value] = m;
      if (prop.startsWith("--")) continue;
      const bad = (why) => findings.push(`${at}: ${prop}: ${value.trim()}  (${why})`);
      if (prop === "font-size" && /\d(px|rem)/.test(value)) bad("use var(--fs-*)");
      if (prop === "font" && /\d+(\.\d+)?px/.test(value)) bad("use var(--fs-*)/var(--lh-*) or a --type-* token");
      if (prop === "font-weight" && !/^(400|500|600|700|inherit|normal)$/.test(value.trim())) bad("weights are 400 / 500 / 600 / 700");
      if (prop === "font" && /^\s*(800|900|300)\b/.test(value)) bad("weights are 400 / 500 / 600 / 700");
      if (prop === "border-radius") {
        for (const px of value.matchAll(/(\d+(?:\.\d+)?)px/g)) if (Number(px[1]) > 2) bad("use var(--r-sm|md|lg|full)");
      }
      if (/^(padding|margin)(-(top|right|bottom|left|inline|block)(-(start|end))?)?$|^(row-|column-)?gap$/.test(prop)) {
        for (const px of value.matchAll(/(-?\d+(?:\.\d+)?)px/g)) if (!GRID.has(Math.abs(Number(px[1])))) bad("off the 4 px grid");
      }
      if (/rgba?\(|#[0-9a-fA-F]{3,8}\b/.test(value) && !/^#(fff|000|ffffff|000000)$/i.test(value.trim().replace(/\s*!important/, ""))) {
        const stripped = value.replace(/#(fff|000)\b/gi, "");
        if (/rgba?\(|#[0-9a-fA-F]{3,8}\b/.test(stripped)) bad("use a colour role (var(--…))");
      }
      if (prop === "box-shadow") {
        const v = value.trim();
        const ok = /^(none|var\(--(shadow-1|shadow-2|shadow-sheet|shadow-cta|focus)\))$/.test(v)
          || (/inset/.test(v) && /var\(--/.test(v) && !/rgba?\(/.test(v))
          || /^0 0 0 \d+(\.\d+)?px var\(--[a-z-]+\)$/.test(v);
        if (!ok) bad("use var(--shadow-*), var(--focus) or none");
      }
    }
  });
}
for (const f of findings) console.log(f);
console.log(`design lint: ${findings.length ? `${findings.length} finding(s)` : "ok"} (${files.length} stylesheets)`);
process.exit(findings.length ? 1 : 0);
