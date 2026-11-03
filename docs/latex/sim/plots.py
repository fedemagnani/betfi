import json, sys
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

r = json.load(open(sys.argv[1])); out = sys.argv[2]
C1, C2, C3 = "#2a78d6", "#eb6834", "#1baf7a"     # categorical slots 1-3, fixed order
INK, MUTED, GRID = "#2b2b2b", "#6b6b6b", "#e4e4e4"
plt.rcParams.update({
    "font.family": "serif", "font.size": 9, "axes.edgecolor": MUTED, "axes.labelcolor": INK,
    "xtick.color": MUTED, "ytick.color": MUTED, "axes.spines.top": False,
    "axes.spines.right": False, "axes.grid": True, "grid.color": GRID, "grid.linewidth": 0.6,
    "legend.frameon": False, "lines.linewidth": 1.6, "savefig.bbox": "tight"})

# Figure 1: residual risk vs rebalancing interval
e2 = r["exp2"]
x = [d["interval"] for d in e2]
fig, ax = plt.subplots(figsize=(4.6, 3.0))
ax.plot(x, [d["std"] for d in e2], "-o", color=C1, ms=5, label=r"Simulated $\mathrm{Std}(\mathcal{E})/N$")
ax.plot(x, [d["floor_std"] for d in e2], "--s", color=C2, ms=4, mfc="white", label="Square root of the floor (Theorem 5.12)")
ax.set_xscale("log"); ax.set_yscale("log")
ax.set_xticks(x); ax.set_xticklabels([f"{v:g}" for v in x])
ax.set_yticks([0.015, 0.03, 0.06, 0.12, 0.24]); ax.set_yticklabels(["0.015", "0.03", "0.06", "0.12", "0.24"])
ax.minorticks_off()
ax.set_xlabel("Rebalancing interval (minutes)"); ax.set_ylabel("Standard deviation of error / payout")
ax.legend(loc="upper left")
fig.savefig(f"{out}/fig_frequency.pdf"); plt.close(fig)

# Figure 2: cost-risk frontier of band policies
e3 = r["exp3"]
fig, ax = plt.subplots(figsize=(4.6, 3.0))
xs = [d["std"] for d in e3]; ys = [d["EL"] for d in e3]
ax.plot(xs, ys, "-o", color=C1, ms=5)
for d in e3:
    if d["eta"] in (0.01, 0.02):
        continue
    ax.annotate(rf"$\eta={d['eta']:g}$", (d["std"], d["EL"]), textcoords="offset points",
                xytext=(6, 3), fontsize=7.5, color=INK)
ax.set_xlabel(r"Standard deviation of error / payout, $\mathrm{Std}(\mathcal{E})/N$")
ax.set_ylabel(r"Expected friction cost / payout, $\mathbb{E}[\mathcal{L}]/N$")
ax.set_xlim(0, max(xs) * 1.18); ax.set_ylim(0, max(ys) * 1.12)
fig.savefig(f"{out}/fig_bands.pdf"); plt.close(fig)

# Figure 3: quoted odds vs stake
e4 = r["exp4"]; S = e4["S"]
fig, ax = plt.subplots(figsize=(4.6, 3.0))
ax.axhline(e4["odd_competitive"], color=MUTED, lw=1.0, ls=":")
ax.text(S[0], e4["odd_competitive"] + 0.06, r"Competitive odd $(1-m)/(p_Ap_B)$", fontsize=7.5, color=MUTED)
ax.plot(S, e4["by_lambda"]["0.25"]["odd"], color=C1, label=r"Dynamic hedge, $\lambda=0.25$")
ax.plot(S, e4["by_lambda"]["1.0"]["odd"], color=C2, label=r"Dynamic hedge, $\lambda=1$")
ax.plot(S, e4["odd_static"], color=C3, label="Static superhedge")
ax.set_xscale("log"); ax.set_xlabel("Stake $S$"); ax.set_ylabel("Quoted odd $o(S)$")
ax.set_ylim(0.8, 5.3)
ticks = [200, 500, 1000, 2000, 5000, 10000, 20000]
ax.set_xticks(ticks); ax.set_xticklabels([f"{t:,}" for t in ticks]); ax.minorticks_off()
ax.legend(loc="lower left")
fig.savefig(f"{out}/fig_odds.pdf"); plt.close(fig)
print("ok")
