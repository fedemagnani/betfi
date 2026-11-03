// What every BetFi screen must hold, measured in the page (6.2a-g; v2 rules by 7.0a). Shared by
// phone_gate.mjs, devnet_e2e.mjs and visual/shoot.mjs so the three agree on one definition:
//   - dark: the page paints a dark background whatever the system prefers (D-310);
//   - no horizontal scroll at the viewport's width;
//   - no trading words or Pascal symbols in the visible text (BANNED); with `noOdds`, not the
//     word "odds" either (guidelines §2; the kit gallery and every v2 screen);
//   - every market toned (guidelines §2, 7.0a): among the values of one market shown side by
//     side, the lowest is Solana green, the highest Solana purple, the others (and every value of
//     an all-equal market) neutral. v2 `.k-outcome` groups of 2 or 3 carry `tone-lowest`,
//     `tone-highest`, `tone-neutral`; the L6 `.k-odds` pairs carry `tone-lower` (green),
//     `tone-higher` (purple), or no tone class when equal or unpriced. A selected button keeps
//     its tone (the selection is the gradient border);
//   - every figure in Plus Jakarta Sans with tabular figures (DESIGN.md): each text node that is
//     only a USDC, SOL or token amount ("157.97 USDC"), or only a value ("1.62"), and every
//     `.num`;
//   - the honesty pills (U-7, D-311) visible on the page, by id.
// `screenFacts(page)` returns the measures; `problems(facts, opts)` the failed ones by name.

/// Words and symbols no screen may show (2.E, L6 common rules).
export const BANNED = [/\blong\b/i, /\bshort\b/i, /order ?book/i, /NBA_\w+/, /\bbid\b/i, /\bask\b/i];
/// Guidelines §2: never the word "odds" (checked where `noOdds` is set).
export const ODDS_WORD = /\bodds\b/i;

/// The tone colours (main.css --sol-green, --sol-purple), as computed RGB.
export const GREEN = "rgb(20, 241, 149)";
export const PURPLE = "rgb(153, 69, 255)";

/// Waits until the page has painted what the last action changed: fonts loaded, then two
/// animation frames (Dioxus renders after the event handler returns). No fixed sleep.
export async function settle(page) {
  await page.evaluate(() => document.fonts.ready.then(() => new Promise((r) =>
    requestAnimationFrame(() => requestAnimationFrame(() => r()))))).catch(() => {});
}

/// The page's measures (see the header). `text` is the visible text.
export async function screenFacts(page) {
  await settle(page);
  return page.evaluate(({ GREEN, PURPLE }) => {
    const visible = (e) => !!e && e.checkVisibility({ visibilityProperty: true, opacityProperty: true });
    // Dark: the first opaque background from body up, and its relative luminance.
    const bgOf = (e) => {
      for (let x = e; x; x = x.parentElement) {
        const c = getComputedStyle(x).backgroundColor;
        const m = c.match(/rgba?\((\d+), (\d+), (\d+)(?:, ([\d.]+))?\)/);
        if (m && (m[4] === undefined || Number(m[4]) > 0.5)) return [Number(m[1]), Number(m[2]), Number(m[3])];
      }
      return [255, 255, 255];
    };
    const lum = ([r, g, b]) => {
      const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
      return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
    };
    const bg = bgOf(document.body);
    const scheme = getComputedStyle(document.documentElement).colorScheme;

    // Markets: an element whose visible children include 2 (L6 `.k-odds`) or 2–3 (v2
    // `.k-outcome`) buttons of the same kind.
    const num = (b, sel) => {
      const n = Number((b.querySelector(sel)?.textContent ?? "").trim());
      return Number.isFinite(n) && n > 0 ? Math.round(n * 100) : 0;
    };
    const untoned = [];
    let pairs = 0;
    for (const [btn, valueSel, sizes, tonesOf] of [
      [".k-odds", ".k-odds-value", [2], (v) => {
        if (v[0] === 0 || v[1] === 0 || v[0] === v[1]) return [null, null];
        return v[0] > v[1] ? ["higher", "lower"] : ["lower", "higher"];
      }],
      [".k-outcome", ".k-outcome-value", [2, 3], (v) => {
        const priced = v.filter((x) => x > 0);
        const lo = Math.min(...priced);
        const hi = Math.max(...priced);
        return v.map((x) => (x > 0 && lo < hi && x === lo ? "lowest" : x > 0 && lo < hi && x === hi ? "highest" : "neutral"));
      }],
    ]) {
      const parents = new Set([...document.querySelectorAll(btn)].filter(visible).map((b) => b.parentElement));
      for (const p of parents) {
        const bs = [...p.children].filter((c) => c.matches(btn) && visible(c));
        if (!sizes.includes(bs.length)) continue;
        pairs++;
        const want = tonesOf(bs.map((b) => num(b, valueSel)));
        bs.forEach((b, i) => {
          const tone = /\btone-(higher|lower|lowest|highest|neutral)\b/.exec(b.className)?.[1] ?? null;
          const color = getComputedStyle(b.querySelector(valueSel) ?? b).color;
          const wantColor = { lower: GREEN, lowest: GREEN, higher: PURPLE, highest: PURPLE }[want[i]];
          const toneOk = tone === want[i] || (want[i] === null && (tone === null || tone === "neutral"));
          // A selected button is solid accent with white text (L9, Revolut): its tone class still
          // counts, its colour gives way to the selection.
          const selected = b.classList.contains("selected") || b.getAttribute("aria-pressed") === "true";
          const colorOk = b.disabled || selected || !wantColor || color === wantColor;
          if (!toneOk || !colorOk) {
            untoned.push(`${b.querySelector(valueSel)?.textContent ?? "?"}: ${tone ?? "no tone"} (want ${want[i] ?? "none"}), ${color}`);
          }
        });
      }
    }

    // Figures: text nodes that are only an amount in USDC, SOL or a token, or only a value; and
    // `.num`. Each must be Plus Jakarta Sans with tabular figures.
    const FIGURE = /^(?:[\d,]+(?:\.\d+)? (?:USDC|mockBONK|BONK|SOL|mockSOL)|\d{1,3}\.\d{2})$/;
    const tabular = (e) => {
      const cs = getComputedStyle(e);
      return /^\s*"?Plus Jakarta Sans"?/.test(cs.fontFamily)
        && (/tabular-nums/.test(cs.fontVariantNumeric) || /"tnum"/.test(cs.fontFeatureSettings));
    };
    const notTabular = [];
    let figures = 0;
    const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n; (n = walk.nextNode());) {
      const t = n.nodeValue.trim();
      if (!FIGURE.test(t) || !visible(n.parentElement)) continue;
      figures++;
      if (!tabular(n.parentElement)) notTabular.push(t);
    }
    for (const e of document.querySelectorAll(".num")) {
      if (visible(e) && e.textContent.trim() && !tabular(e)) notTabular.push(`.num ${e.textContent.trim().slice(0, 20)}`);
    }

    const pills = [...document.querySelectorAll(".k-honesty")].filter(visible)
      .map((e) => ({ id: e.id || null, mock: /mock-filled/.test(e.innerText) }));
    return {
      width: innerWidth,
      scrollWidth: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth),
      text: document.body.innerText,
      dark: lum(bg) < 0.05 && /dark/.test(scheme),
      bg: `rgb(${bg.join(", ")})`,
      pairs, untoned: untoned.slice(0, 6), figures, notTabular: [...new Set(notTabular)].slice(0, 6), pills,
    };
  }, { GREEN, PURPLE });
}

/// The failed checks of `facts` (see the header), by name; empty when the screen is clean.
/// `toned: false` skips the market tones (the gallery shows hand-set tones on purpose);
/// `noOdds: true` also bans the word "odds".
export function problems(facts, { toned = true, noOdds = false } = {}) {
  const out = [];
  if (!facts.dark) out.push(`not dark (${facts.bg})`);
  if (facts.scrollWidth > facts.width) out.push(`horizontal scroll (${facts.scrollWidth} > ${facts.width})`);
  const banned = BANNED.filter((r) => r.test(facts.text)).map(String);
  if (banned.length) out.push(`trading words ${banned.join(" ")}`);
  if (noOdds && ODDS_WORD.test(facts.text)) out.push(`the word "odds" (guidelines §2)`);
  if (toned && facts.untoned.length) out.push(`values not toned: ${facts.untoned.join("; ")}`);
  if (facts.notTabular.length) out.push(`figures not in tabular Plus Jakarta Sans: ${facts.notTabular.join(", ")}`);
  return out;
}
