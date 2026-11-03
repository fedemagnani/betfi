"""Draws the architecture as an isometric scene with animated data flows: docs/architecture.svg
(for the book and GitHub) and the same file in slides/public/ (for the Slidev deck).

    python3 scripts/architecture.py
"""
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
U = 30  # pixels per grid unit
C30, S30 = math.cos(math.pi / 6), 0.5

# Tier colours: (top, left face, right face, text on top).
CLIENT = ("#9945ff", "#7a2fe0", "#5f22b5", "#ffffff")
DESK_SLAB = ("#1b2436", "#141b29", "#0f1520", "#8aa0c8")
DESK = ("#3478f6", "#2259c2", "#1a479c", "#ffffff")
CHAIN_SLAB = ("#17271f", "#111d17", "#0c1611", "#7fbf9f")
DB = ("#d5dcea", "#99a5bc", "#7d89a0", "#141a26")
RAIL = ("#334566", "#26344f", "#1e2a40", "#c4d2ee")
CHAIN = ("#14f195", "#0fbf76", "#0b965c", "#04261a")
PASCAL = ("#f2a93b", "#c27d1c", "#9c6413", "#2b1a02")
EXTERNAL = ("#3a3f4b", "#2a2e37", "#21242b", "#e6e8ee")
BG, GRID, INK, MUTED = "#0a0b10", "#151722", "#e6e8ee", "#8b93a5"


def iso(x, y, z=0.0):
    return ((x - y) * C30 * U, (x + y) * S30 * U - z * U)


def pts(points):
    return " ".join(f"{px:.1f},{py:.1f}" for px, py in (iso(*p) for p in points))


class Box:
    def __init__(self, x, y, w, d, h, colours, title, sub="", z=0.0, ghost=False, size=13):
        self.x, self.y, self.w, self.d, self.h, self.z = x, y, w, d, h, z
        self.c, self.title, self.sub, self.ghost, self.size = colours, title, sub, ghost, size
        self.children = []
        self.badge = None

    def top_centre(self):
        return (self.x + self.w / 2, self.y + self.d / 2, self.z + self.h)

    def anchor(self, toward):
        # The top face's corner nearest the other end, pulled a third of the way to the centre,
        # so that arcs leave clear of the label.
        cx, cy, cz = self.top_centre()
        tx, ty = iso(*toward)
        corners = [(self.x + dx, self.y + dy, cz) for dx in (0, self.w) for dy in (0, self.d)]
        bx, by, _ = min(corners, key=lambda c: math.dist(iso(*c), (tx, ty)))
        return (bx + (cx - bx) / 3, by + (cy - by) / 3, cz)

    def svg(self):
        x, y, z, w, d, h = self.x, self.y, self.z, self.w, self.d, self.h
        top, left, right, ink = self.c
        faces = [
            (left, [(x, y + d, z), (x + w, y + d, z), (x + w, y + d, z + h), (x, y + d, z + h)]),
            (right, [(x + w, y, z), (x + w, y + d, z), (x + w, y + d, z + h), (x + w, y, z + h)]),
            (top, [(x, y, z + h), (x + w, y, z + h), (x + w, y + d, z + h), (x, y + d, z + h)]),
        ]
        style = ' fill="none" stroke="#5b6170" stroke-dasharray="5 4"' if self.ghost else ""
        out = []
        for fill, quad in faces:
            paint = style or f' fill="{fill}" stroke="{BG}" stroke-width="0.8"'
            out.append(f'<polygon points="{pts(quad)}"{paint}/>')
        if self.title:
            out.append(self.label())
        if self.badge:
            out.append(self.badge.svg())
        return "\n".join(out)

    def label(self):
        # Text lies on the top face, running along the x axis.
        cx, cy, cz = self.top_centre()
        tx, ty = iso(cx, cy, cz)
        ink = MUTED if self.ghost else self.c[3]
        lines = [f'<text class="t" font-size="{self.size}" y="{-2 if self.sub else 4}" fill="{ink}">{self.title}</text>']
        if self.sub:
            lines.append(f'<text class="s" font-size="{self.size - 3.5}" y="{self.size - 1}" fill="{ink}" opacity="0.85">{self.sub}</text>')
        return (f'<g transform="matrix({C30:.4f},{S30},{-C30:.4f},{S30},{tx:.1f},{ty:.1f})">'
                + "".join(lines) + "</g>")


class Cylinder:
    def __init__(self, cx, cy, r, h, z, colours, title):
        self.cx, self.cy, self.r, self.h, self.z, self.c, self.title = cx, cy, r, h, z, colours, title
        self.x, self.y, self.w, self.d = cx - r, cy - r, 2 * r, 2 * r

    def top_centre(self):
        return (self.cx, self.cy, self.z + self.h)

    def anchor(self, toward):
        return self.top_centre()

    def svg(self):
        top, left, _, ink = self.c
        rx, ry = self.r * math.sqrt(2) * C30 * U, self.r * math.sqrt(2) * S30 * U
        bx, by = iso(self.cx, self.cy, self.z)
        tx, ty = iso(self.cx, self.cy, self.z + self.h)
        body = (f'M{bx - rx:.1f},{ty:.1f} L{bx - rx:.1f},{by:.1f} '
                f'A{rx:.1f},{ry:.1f} 0 0 0 {bx + rx:.1f},{by:.1f} L{bx + rx:.1f},{ty:.1f} Z')
        return (f'<path d="{body}" fill="{left}" stroke="{BG}" stroke-width="0.8"/>'
                f'<ellipse cx="{tx:.1f}" cy="{ty:.1f}" rx="{rx:.1f}" ry="{ry:.1f}" fill="{top}" stroke="{BG}" stroke-width="0.8"/>'
                + (f'<text class="t" x="{tx:.1f}" y="{ty + 4:.1f}" font-size="11" fill="{ink}">{self.title}</text>' if self.title else ""))


# --- the scene -----------------------------------------------------------------------------

app = Box(-1.6, 6.4, 2.4, 2.4, 2.8, CLIENT, "BetFi app", "Dioxus · wasm", size=13)
privy = Box(-5.2, 9.6, 2.4, 2.2, 0.8, EXTERNAL, "Privy", "Google → wallet", size=12)

# The desk: Pascal-facing modules on the back row, chain-facing ones in front, the event bus
# between them ending in SQLite. A small cylinder on a module means it writes to SQLite.
desk = Box(4.8, 1.4, 20.6, 8.8, 0.35, DESK_SLAB, "")
COLS, ROWS = (5.4, 9.8, 14.2, 18.6), (2.0, 7.4)
mods = {}
for key, (col, row, title, sub, writes) in {
    "social": (0, 0, "social", "tipsters · profiles · fx", True),
    "catalogue": (1, 0, "catalogue", "Pascal markets", True),
    "quoter": (2, 0, "quoter", "prices · signs", True),
    "hedge": (3, 0, "hedge desk", "IOC · roll · unwind", True),
    "swap": (0, 1, "swap", "pool quotes", False),
    "relay": (1, 1, "relay", "co-signs · sends", False),
    "indexer": (2, 1, "indexer", "logs → DB → bus", True),
    "crank": (3, 1, "crank", "sends program txs", True),
}.items():
    m = Box(COLS[col], ROWS[row], 3.6, 2.2, 0.75, DESK, title, sub, z=0.35, size=12)
    if writes:
        m.badge = Cylinder(m.x + m.w - 0.42, m.y + 0.42, 0.26, 0.26, m.z + m.h, DB, "")
    mods[key] = m
rail = Box(COLS[1], 5.1, COLS[3] + 3.6 - COLS[1], 0.8, 0.14, RAIL, "", z=0.35)
db = Cylinder(rail.x + rail.w + 1.9, rail.y + rail.d / 2, 1.05, 0.85, 0.35, DB, "SQLite")
desk.children = list(mods.values()) + [rail, db]

mock = Box(28.4, 2.4, 2.8, 2.4, 1.3, EXTERNAL, "pascal-mock", "Pascal's API", size=12)
pascal = Box(33.0, 0.4, 3.2, 3.0, 1.8, PASCAL, "Pascal", "books · results", size=14)

chain = Box(5.0, 13.4, 17.0, 4.0, 0.35, CHAIN_SLAB, "")
amm = Box(5.6, 13.9, 3.2, 3.0, 0.9, CHAIN, "mock-amm", "pools · faucet", z=0.35, size=12)
schedina = Box(11.4, 13.9, 6.4, 3.0, 1.4, CHAIN, "schedina", "escrow · vault · tickets", z=0.35, size=14)
chain.children = [amm, schedina]
aggr = Box(0.2, 13.9, 3.8, 3.0, 0.9, EXTERNAL, "0x · Jupiter · DFlow", "mainnet", ghost=True, size=11)


class Point:
    """A spot on the event bus, as an arc end."""

    def __init__(self, x, y, z):
        self.p = (x, y, z)

    def top_centre(self):
        return self.p

    def anchor(self, toward):
        return self.p


def on_rail(m):
    return Point(m.x + m.w / 2, rail.y + rail.d / 2, rail.z + rail.h)


# (source, target, label, colour, height of the arc in grid units, where the label sits on it)
FLOWS = [
    (privy, app, "Google login", MUTED, 1.4, 0.5),
    (app, mods["quoter"], "① quote · card · live feed", CLIENT[0], 3.6, 0.5),
    (app, mods["social"], "tips · profiles", CLIENT[0], 1.2, 0.5),
    (app, mods["swap"], "swap quote", CLIENT[0], 0.8, 0.55),
    (app, mods["relay"], "② user-signed tx", CLIENT[0], 1.6, 0.6),
    (mods["relay"], schedina, "② + sponsor signature", DESK[0], 0.9, 0.7),
    (schedina, mods["indexer"], "③ program logs", CHAIN[0], 2.2, 0.62),
    (mods["crank"], schedina, "⑤ finalize · ⑥ resolve · settle", DESK[0], 1.0, 0.6),
    (amm, mods["swap"], "pool reserves", CHAIN[0], 0.9, 0.5),
    (mods["hedge"], mock, "④ IOC + builder code", DESK[0], 2.4, 0.5),
    (mock, mods["catalogue"], "markets", MUTED, 4.4, 0.5),
    (mock, mods["quoter"], "books", MUTED, 3.0, 0.5),
    (mock, mods["crank"], "⑥ results", MUTED, 0.8, 0.22),
    (pascal, mock, "live books · results", PASCAL[0], 1.8, 0.5),
]

# In-process events: (source, target, both ways)
BUS = [
    (mods["catalogue"], on_rail(mods["catalogue"]), False),
    (on_rail(mods["quoter"]), mods["quoter"], False),
    (mods["hedge"], on_rail(mods["hedge"]), True),
    (mods["indexer"], on_rail(mods["indexer"]), False),
    (on_rail(mods["crank"]), mods["crank"], False),
]

SLAB_LABELS = [
    (desk, "desk · Rust · tokio · axum", DESK_SLAB[3], "right"),
    (chain, "Solana · Anchor programs", CHAIN_SLAB[3], "left"),
]

LIFECYCLE = [
    "① quote priced on Pascal's books, signed",
    "② user signs open_pending, the sponsor co-signs",
    "③ indexer sees the escrow: writes it, publishes it",
    "④ hedge desk buys every leg on Pascal",
    "⑤ crank sends finalize: the ticket is minted",
    "⑥ Pascal's result: resolve_leg, then settle",
]


def arc(src, dst, lift):
    a, b = iso(*src.anchor(dst.top_centre())), iso(*dst.anchor(src.top_centre()))
    mx, my = (a[0] + b[0]) / 2, min(a[1], b[1]) - lift * U
    return a, (mx, my), b


def path(a, c, b):
    return f"M{a[0]:.1f},{a[1]:.1f} Q{c[0]:.1f},{c[1]:.1f} {b[0]:.1f},{b[1]:.1f}"


def flows_svg(animated):
    out = []
    for src, dst, both in BUS:
        d = path(*arc(src, dst, 0.7))
        start = ' marker-start="url(#arrow)"' if both else ""
        out.append(f'<path class="bus" d="{d}" stroke="{RAIL[3]}" marker-end="url(#arrow)"{start}/>')
    for i, (src, dst, text, colour, lift, _) in enumerate(FLOWS):
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


def rail_label():
    # On the rail's top face, along x.
    tx, ty = iso(rail.x + 0.3, rail.y + rail.d / 2, rail.z + rail.h)
    return (f'<g transform="matrix({C30:.4f},{S30},{-C30:.4f},{S30},{tx:.1f},{ty:.1f})">'
            f'<text class="r" y="3.5" fill="{RAIL[3]}">event bus · PendingOpened · HedgeFilled · TicketMinted · '
            f'LegResolved · RefundNeeded · …</text></g>')


def legend(x, y):
    out = [f'<text class="lh" x="{x}" y="{y}" fill="{INK}">Bet lifecycle</text>']
    for i, line in enumerate(LIFECYCLE):
        out.append(f'<text class="l" x="{x}" y="{y + 20 + 17 * i}" fill="{MUTED}">{line}</text>')
    y += 20 + 17 * len(LIFECYCLE) + 14
    out.append(f'<path class="flow" d="M{x},{y - 4} h30" stroke="{INK}"/>'
               f'<text class="l" x="{x + 40}" y="{y}" fill="{MUTED}">HTTP · WebSocket · RPC</text>')
    out.append(f'<path class="bus" d="M{x},{y + 13} h30" stroke="{RAIL[3]}"/>'
               f'<text class="l" x="{x + 40}" y="{y + 17}" fill="{MUTED}">in-process event bus</text>')
    bx, by = x + 15, y + 30
    rx, ry = 7.5, 4.2
    out.append(f'<path d="M{bx - rx},{by} v7 a{rx},{ry} 0 0 0 {2 * rx},0 v-7 z" fill="{DB[1]}"/>'
               f'<ellipse cx="{bx}" cy="{by}" rx="{rx}" ry="{ry}" fill="{DB[0]}"/>'
               f'<text class="l" x="{x + 40}" y="{y + 36}" fill="{MUTED}">writes to SQLite</text>')
    return "\n".join(out)


def scene(animated):
    items = [app, privy, desk, mock, pascal, chain, aggr]
    items.sort(key=lambda b: b.x + b.y)
    body = []
    for x in range(-6, 34, 2):
        body.append(f'<polyline points="{pts([(x, -2), (x, 22)])}" stroke="{GRID}" fill="none"/>')
    for y in range(-2, 23, 2):
        body.append(f'<polyline points="{pts([(-6, y), (33, y)])}" stroke="{GRID}" fill="none"/>')
    for item in items:
        body.append(item.svg())
        for child in sorted(item.children, key=lambda b: (b.y, b.x)):
            body.append(child.svg())
            if child is rail:
                body.append(rail_label())
    for slab, text, colour, edge in SLAB_LABELS:
        if edge == "left":  # along the front-left edge, running down-right
            ex, ey = iso(slab.x + 0.3, slab.y + slab.d, 0.1)
            m = f"{C30:.4f},{S30},0,1"
        else:  # along the front-right edge, running up-right
            ex, ey = iso(slab.x + slab.w, slab.y + slab.d - 0.3, 0.1)
            m = f"{C30:.4f},{-S30},0,1"
        body.append(f'<g transform="matrix({m},{ex:.1f},{ey:.1f})">'
                    f'<text class="e" y="14" fill="{colour}">{text}</text></g>')
    body.append(flows_svg(animated))

    xs, ys = [], []
    for item in items + desk.children + chain.children:
        for dx in (0, item.w):
            for dy in (0, item.d):
                for dz in (0, getattr(item, "h", 0) + getattr(item, "z", 0)):
                    px, py = iso(item.x + dx, item.y + dy, dz)
                    xs.append(px)
                    ys.append(py)
    for src, dst, _, _, lift, _ in FLOWS:
        a, c, b = arc(src, dst, lift)
        ys.append((a[1] + 2 * c[1] + b[1]) / 4 - 12)
    pad = 36
    x0, y0 = min(xs) - pad, min(ys) - pad - 40
    w, h = max(xs) - x0 + pad, max(ys) - y0 + pad
    lx, ly = x0 + w - 330, y0 + 40  # the empty corner above Pascal

    motion = """
  .flow, .bus { animation: dash 1.2s linear infinite; }
  @keyframes dash { to { stroke-dashoffset: -24; } }
  @media (prefers-reduced-motion: reduce) { .flow, .bus { animation: none; } circle { display: none; } }""" if animated else ""
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x0:.0f} {y0:.0f} {w:.0f} {h:.0f}" width="{w:.0f}" height="{h:.0f}" role="img" aria-label="BetFi architecture: the app, the desk's modules around its event bus, pascal-mock and Pascal, and the Solana programs, with the bet lifecycle numbered">
<style>
  text {{ font-family: Inter, -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif; text-anchor: middle; }}
  .t {{ font-weight: 650; }}
  .s {{ font-weight: 450; }}
  .f {{ font-size: 10.5px; font-weight: 500; }}
  .r {{ font-size: 9px; font-weight: 600; text-anchor: start; letter-spacing: 0.3px; }}
  .e {{ font-size: 11.5px; font-weight: 600; text-anchor: start; letter-spacing: 0.4px; }}
  .h {{ font-size: 17px; font-weight: 700; text-anchor: start; }}
  .hs {{ font-size: 11.5px; text-anchor: start; }}
  .lh {{ font-size: 12.5px; font-weight: 700; text-anchor: start; }}
  .l {{ font-size: 11px; text-anchor: start; }}
  .flow {{ fill: none; stroke-width: 1.6; stroke-dasharray: 5 7; opacity: 0.9; }}
  .bus {{ fill: none; stroke-width: 1.4; stroke-dasharray: 1.5 4; stroke-linecap: round; }}{motion}
</style>
<defs><marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
  <path d="M0,1 L9,5 L0,9 z" fill="context-stroke"/></marker></defs>
<rect x="{x0:.0f}" y="{y0:.0f}" width="{w:.0f}" height="{h:.0f}" fill="{BG}" rx="14"/>
{chr(10).join(body)}
<rect x="{x0 + 12:.0f}" y="{y0 + 12:.0f}" width="460" height="52" fill="{BG}"/>
<text class="h" x="{x0 + 24:.0f}" y="{y0 + 34:.0f}" fill="{INK}">BetFi · architecture</text>
<text class="hs" x="{x0 + 24:.0f}" y="{y0 + 52:.0f}" fill="{MUTED}">full Rust stack: Dioxus app · tokio/axum desk · Anchor programs</text>
{legend(round(lx), round(ly))}
</svg>
"""


def main():
    for svg in (ROOT / "docs" / "architecture.svg", ROOT / "slides" / "public" / "architecture.svg"):
        svg.parent.mkdir(parents=True, exist_ok=True)
        svg.write_text(scene(animated=True))
        print(f"wrote {svg.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
