"""Draws the bet lifecycle as an isometric scene, in the architecture drawing's style:
docs/lifecycle.svg (animated, for the book and GitHub) and the same file in slides/public/ (for
the Slidev deck).

Only the parts that act on a bet are drawn: the user's app, the desk modules a bet goes
through, the schedina program, pascal-mock and Pascal. The arcs are what each one sends, numbered
as the steps in docs/lifecycle.md; outcomes (minted, refunded, settled, cashed out) are labels
on the arcs, never boxes.

    python3 scripts/lifecycle.py
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from architecture import (  # noqa: E402  (the shared isometric kit)
    BG, C30, CHAIN, CHAIN_SLAB, CLIENT, DESK, DESK_SLAB, EXTERNAL, GRID, INK, MUTED, PASCAL,
    RAIL, S30, Box, arc, iso, path, pts,
)

ROOT = Path(__file__).resolve().parent.parent

# --- the scene -----------------------------------------------------------------------------

app = Box(-7.0, 8.6, 2.6, 2.6, 2.8, CLIENT, "BetFi app", "the user signs", size=13)

# The desk: the quoter and the hedge desk face Pascal (back row); the relay, the indexer and
# the crank face the chain (front row).
desk = Box(-0.6, 0.6, 20.8, 8.6, 0.35, DESK_SLAB, "")
quoter = Box(0.4, 1.6, 3.8, 2.2, 0.75, DESK, "quoter", "prices · signs", z=0.35, size=12)
hedge = Box(9.4, 1.6, 3.8, 2.2, 0.75, DESK, "hedge desk", "buys every leg", z=0.35, size=12)
relay = Box(0.4, 6.0, 3.8, 2.2, 0.75, DESK, "relay", "co-signs · pays the fee", z=0.35, size=12)
indexer = Box(6.4, 6.0, 3.8, 2.2, 0.75, DESK, "indexer", "program logs → bus", z=0.35, size=12)
crank = Box(12.6, 6.0, 3.8, 2.2, 0.75, DESK, "crank", "the operator key", z=0.35, size=12)
desk.children = [quoter, hedge, relay, indexer, crank]

mock = Box(19.6, -6.0, 2.8, 2.4, 1.3, EXTERNAL, "pascal-mock", "Pascal's API", size=12)
pascal = Box(23.6, -10.4, 3.2, 3.0, 1.8, PASCAL, "Pascal", "books · results", size=14)

chain = Box(0.4, 12.4, 16.6, 4.0, 0.35, CHAIN_SLAB, "")
schedina = Box(1.6, 12.9, 12.4, 3.0, 1.4, CHAIN, "schedina", "escrow · vault · tickets", z=0.35, size=14)
chain.children = [schedina]

# What each part sends: (source, target, label, colour, arc height in grid units, label position).
FLOWS = [
    (quoter, app, "① signed quote · valid 10 s", DESK[0], 2.6, 0.45),
    (app, relay, "② open_pending, user-signed", CLIENT[0], 1.2, 0.62),
    (relay, schedina, "② + sponsor: stake → escrow, 30–60 s", DESK[0], 1.0, 0.55),
    (schedina, indexer, "③ program logs", CHAIN[0], 1.6, 0.3),
    (hedge, mock, "③ IOC at the user's max prices", DESK[0], 2.4, 0.5),
    (crank, schedina, "④ finalize: payout locked · or refund_pending", DESK[0], 1.2, 0.7),
    (mock, crank, "⑤ results", MUTED, 1.0, 0.45),
    (schedina, app, "⑥ settle, cash-out or refund → the wallet", CHAIN[0], 1.4, 0.42),
    (pascal, mock, "live books · results", PASCAL[0], 1.8, 0.5),
]

# In-process events between desk modules: (source, target, label, label offset in px).
BUS = [(indexer, hedge, "PendingOpened", (0, -8)), (hedge, crank, "HedgeFilled", (44, 10))]

# The crank's later sends ride the finalize arc; their step goes in the legend.
LEGEND = [
    ("① Quote", "the quoter prices the slip on Pascal's books and signs it"),
    ("② Escrow", "the user signs open_pending; the relay co-signs and pays the fee"),
    ("③ Hedge", "the indexer sees the escrow; the hedge desk buys every leg"),
    ("④ Mint", "the crank sends finalize: ticket minted, payout locked in the vault"),
    ("", "nothing filled: refund_pending, 15 s after expiry"),
    ("⑤ Results", "the crank reads Pascal's results and sends resolve_leg"),
    ("⑥ Payout", "settle pays from the vault, or the user cashes out earlier"),
]


def flows_svg(animated):
    out = []
    for src, dst, text, (dx, dy) in BUS:
        a, c, b = arc(src, dst, 0.9)
        out.append(f'<path class="bus" d="{path(a, c, b)}" stroke="{RAIL[3]}" marker-end="url(#arrow)"/>')
        out.append(f'<text class="f" x="{c[0] + dx:.1f}" y="{(a[1] + 2 * c[1] + b[1]) / 4 + dy:.1f}" fill="{RAIL[3]}">{text}</text>')
    for i, (src, dst, _, colour, lift, _) in enumerate(FLOWS):
        d = path(*arc(src, dst, lift))
        out.append(f'<path id="f{i}" class="flow" d="{d}" stroke="{colour}" marker-end="url(#arrow)"/>')
        if animated:
            out.append(f'<circle r="3.2" fill="{colour}"><animateMotion dur="{2.6 + 0.3 * (i % 4):.1f}s" '
                       f'repeatCount="indefinite" begin="{-0.4 * i:.1f}s"><mpath href="#f{i}"/></animateMotion></circle>')
    for src, dst, text, colour, lift, t in FLOWS:
        a, c, b = arc(src, dst, lift)
        u = 1 - t
        lx = u * u * a[0] + 2 * u * t * c[0] + t * t * b[0]
        ly = u * u * a[1] + 2 * u * t * c[1] + t * t * b[1]
        width = 6.1 * len(text) + 12
        out.append(f'<rect x="{lx - width / 2:.1f}" y="{ly - 9:.1f}" width="{width:.1f}" height="16" rx="8" '
                   f'fill="{BG}" stroke="{colour}" stroke-opacity="0.55"/>'
                   f'<text class="f" x="{lx:.1f}" y="{ly + 3:.1f}" fill="{INK}">{text}</text>')
    return "\n".join(out)


def legend(x, y):
    out = [f'<text class="lh" x="{x}" y="{y}" fill="{INK}">Steps</text>']
    for i, (step, line) in enumerate(LEGEND):
        yy = y + 20 + 17 * i
        out.append(f'<text class="lb" x="{x}" y="{yy}" fill="{INK}">{step}</text>'
                   f'<text class="l" x="{x + 70}" y="{yy}" fill="{MUTED}">{line}</text>')
    y += 20 + 17 * len(LEGEND) + 14
    out.append(f'<path class="flow" d="M{x},{y - 4} h30" stroke="{INK}"/>'
               f'<text class="l" x="{x + 40}" y="{y}" fill="{MUTED}">HTTP · RPC · transaction</text>')
    out.append(f'<path class="bus" d="M{x},{y + 13} h30" stroke="{RAIL[3]}"/>'
               f'<text class="l" x="{x + 40}" y="{y + 17}" fill="{MUTED}">in-process event bus</text>')
    return "\n".join(out)


def scene(animated):
    items = [app, desk, mock, pascal, chain]
    items.sort(key=lambda b: b.x + b.y)
    body = []
    for gx in range(-8, 34, 2):
        body.append(f'<polyline points="{pts([(gx, -12), (gx, 19)])}" stroke="{GRID}" fill="none"/>')
    for gy in range(-12, 20, 2):
        body.append(f'<polyline points="{pts([(-8, gy), (33, gy)])}" stroke="{GRID}" fill="none"/>')
    for item in items:
        body.append(item.svg())
        for child in sorted(item.children, key=lambda b: (b.y, b.x)):
            body.append(child.svg())
    ex, ey = iso(chain.x + 0.3, chain.y + chain.d, 0.1)
    body.append(f'<g transform="matrix({C30:.4f},{S30},0,1,{ex:.1f},{ey:.1f})">'
                f'<text class="e" y="14" fill="{CHAIN_SLAB[3]}">Solana · Anchor program</text></g>')
    ex, ey = iso(desk.x + desk.w, desk.y + desk.d - 0.3, 0.1)
    body.append(f'<g transform="matrix({C30:.4f},{-S30},0,1,{ex:.1f},{ey:.1f})">'
                f'<text class="e" y="14" fill="{DESK_SLAB[3]}">desk · one process</text></g>')
    body.append(flows_svg(animated))

    xs, ys = [], []
    for item in items + desk.children + chain.children:
        for dx in (0, item.w):
            for dy in (0, item.d):
                for dz in (0, item.h + item.z):
                    px, py = iso(item.x + dx, item.y + dy, dz)
                    xs.append(px)
                    ys.append(py)
    for src, dst, _, _, lift, _ in FLOWS:
        a, c, b = arc(src, dst, lift)
        ys.append((a[1] + 2 * c[1] + b[1]) / 4 - 12)
    pad = 36
    x0, y0 = min(xs) - pad, min(ys) - pad - 40
    w, h = max(xs) - x0 + pad, max(ys) - y0 + pad
    lx, ly = x0 + w - 470, y0 + h - 200  # the empty corner right of the program

    motion = """
  .flow, .bus { animation: dash 1.2s linear infinite; }
  @keyframes dash { to { stroke-dashoffset: -24; } }
  @media (prefers-reduced-motion: reduce) { .flow, .bus { animation: none; } circle { display: none; } }""" if animated else ""
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x0:.0f} {y0:.0f} {w:.0f} {h:.0f}" width="{w:.0f}" height="{h:.0f}" role="img" aria-label="BetFi bet lifecycle: the quoter signs a quote, the user's open_pending goes through the relay into the schedina program's escrow, the indexer sees it, the hedge desk buys on Pascal through pascal-mock, the crank finalizes or refunds, then resolves and settles from Pascal's results">
<style>
  text {{ font-family: Inter, -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif; text-anchor: middle; }}
  .t {{ font-weight: 650; }}
  .s {{ font-weight: 450; }}
  .f {{ font-size: 10.5px; font-weight: 500; }}
  .e {{ font-size: 11.5px; font-weight: 600; text-anchor: start; letter-spacing: 0.4px; }}
  .h {{ font-size: 17px; font-weight: 700; text-anchor: start; }}
  .hs {{ font-size: 11.5px; text-anchor: start; }}
  .lh {{ font-size: 12.5px; font-weight: 700; text-anchor: start; }}
  .lb {{ font-size: 11px; font-weight: 600; text-anchor: start; }}
  .l {{ font-size: 11px; text-anchor: start; }}
  .flow {{ fill: none; stroke-width: 1.6; stroke-dasharray: 5 7; opacity: 0.9; }}
  .bus {{ fill: none; stroke-width: 1.4; stroke-dasharray: 1.5 4; stroke-linecap: round; }}{motion}
</style>
<defs><marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
  <path d="M0,1 L9,5 L0,9 z" fill="context-stroke"/></marker></defs>
<rect x="{x0:.0f}" y="{y0:.0f}" width="{w:.0f}" height="{h:.0f}" fill="{BG}" rx="14"/>
{chr(10).join(body)}
<rect x="{x0 + 12:.0f}" y="{y0 + 12:.0f}" width="460" height="52" fill="{BG}"/>
<text class="h" x="{x0 + 24:.0f}" y="{y0 + 34:.0f}" fill="{INK}">BetFi · bet lifecycle</text>
<text class="hs" x="{x0 + 24:.0f}" y="{y0 + 52:.0f}" fill="{MUTED}">which part sends what, from the quote to the payout</text>
{legend(round(lx), round(ly))}
</svg>
"""


def main():
    for svg in (ROOT / "docs" / "lifecycle.svg", ROOT / "slides" / "public" / "lifecycle.svg"):
        svg.parent.mkdir(parents=True, exist_ok=True)
        svg.write_text(scene(animated=True))
        print(f"wrote {svg.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
