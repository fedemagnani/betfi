#!/usr/bin/env python3
"""Team logos for the app: one small PNG per team, plus the index the app embeds.

Reads the Brandfetch export at <src>/index.json (`assets/team-logos/` at the repo root: one
entry per team with its league, its folder and the files it has; 3 Nov: the Champions League
clubs the export lacked are Wikipedia's crests, `"source": "wikipedia"`), picks one raster per team,
shrinks it with macOS `sips` to at most 96 px on its longest side, and writes

  app/public/teams/<league>/<team>.png    served at /teams/<league>/<team>.png (dx copies public/)
  app/src/ui/kit/team_logos.json          [{"team", "league", "sport", "path"}], embedded by kit/logos.rs

Pick order, for a dark app: a "logo" for dark backgrounds, then a dark "symbol", then a dark
"icon" (square, often on its own background), then a light logo, then anything raster. SVG is
never needed: every team in the export has a raster file.

Usage, from app/:  python3 -I scripts/gen_team_logos.py ../assets/team-logos
"""
import json
import pathlib
import shutil
import subprocess
import sys

RASTER = (".png", ".jpeg", ".jpg", ".webp")
SPORT = {
    "NBA": "basketball", "Premier League": "soccer", "LaLiga": "soccer", "Bundesliga": "soccer",
    "Champions League": "soccer",
}
MAX_PX = "96"
# Teams whose best-ranked file reads badly on the dark badge (checked on a contact sheet).
OVERRIDE = {}


def aspect(f):
    """Width / height from the file name ("logo-dark-800x213.png"), 1.0 when unknown."""
    import re
    m = re.search(r"-(\d+)x(\d+)\.", f["file"])
    return int(m.group(1)) / int(m.group(2)) if m else 1.0


def rank(f):
    """Lower is better. A round badge needs a roughly square image: a wide wordmark (Tottenham's
    800x108) or a tall one shows as a sliver, so the shape counts first, then the kind."""
    order = [("logo", "dark"), ("symbol", "dark"), ("icon", "dark"), ("logo", "light"), ("symbol", "light")]
    key = (f.get("type"), f.get("theme"))
    a = aspect(f)
    shape = 0 if 0.6 <= a <= 1.6 else 1
    return (shape, order.index(key) if key in order else len(order))


def main():
    src = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "../assets/team-logos").resolve()
    app = pathlib.Path(__file__).resolve().parent.parent
    out_dir = app / "public" / "teams"
    index_out = app / "src" / "ui" / "kit" / "team_logos.json"
    teams = json.loads((src / "index.json").read_text())
    if out_dir.exists():
        shutil.rmtree(out_dir)
    entries, skipped = [], []
    for t in teams:
        files = [f for f in t["files"] if f["file"].lower().endswith(RASTER)]
        if not files:
            skipped.append(t["team"])
            continue
        best = OVERRIDE.get(t["team"]) and next((f for f in files if f["file"] == OVERRIDE[t["team"]]), None)
        best = best or sorted(files, key=lambda f: (rank(f), -f.get("bytes", 0)))[0]
        rel = pathlib.Path(t["dir"])  # e.g. premier-league/arsenal-fc
        dest = out_dir / rel.parent / (rel.name + ".png")
        dest.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["sips", "-s", "format", "png", "-Z", MAX_PX, str(src / rel / best["file"]), "--out", str(dest)],
            check=True, capture_output=True,
        )
        entries.append({
            "team": t["team"],
            "league": t["league"],
            "sport": SPORT.get(t["league"], "soccer"),
            "path": "/teams/" + str(rel.parent) + "/" + rel.name + ".png",
        })
    entries.sort(key=lambda e: (e["league"], e["team"]))
    index_out.write_text(json.dumps(entries, indent=1, ensure_ascii=False) + "\n")
    total = sum(p.stat().st_size for p in out_dir.rglob("*.png"))
    print(f"{len(entries)} logos, {total // 1024} KB in {out_dir}; index {index_out}; skipped {skipped or 'none'}")


if __name__ == "__main__":
    main()
