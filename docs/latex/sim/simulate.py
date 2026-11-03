"""
Numerical illustration for 'Pricing Parlays by Hedging on Prediction-Market Order Books'.

Two independent soccer-like matches. Each match's goal difference Z follows a
difference of two Poisson processes; leg E = {home team wins} = {Z_T > 0}.
Fair value m_t = P(Z_T > 0 | Z_t), computed with the Skellam distribution, is an
exact martingale ending at the outcome. Order books have mid = fair value,
half-spread s (widened after a goal) and linear impact kappa.
"""
import json, sys
import numpy as np
from scipy.stats import skellam

rng = np.random.default_rng(20261101)

T = 90.0                     # minutes
DT = 0.25                    # base grid (minutes)
NSTEP = int(T / DT)          # 360
NPATH = 20000
ZMAX = 12

GAMES = {
    "A": dict(home=1.55, away=1.05),   # expected goals over the match
    "B": dict(home=1.35, away=1.25),
}

S0 = 0.01          # baseline half-spread
WIDEN = 3.0        # spread multiplier after a goal in the same game
WIDEN_MIN = 3.0    # minutes the spread stays wide after a goal
KAPPA = 5e-6       # linear impact coefficient (per contract^2)


def fair_value_table(home, away):
    zs = np.arange(-ZMAX, ZMAX + 1)
    tab = np.empty((NSTEP + 1, zs.size))
    for i in range(NSTEP + 1):
        tau = T - i * DT
        if tau <= 1e-12:
            tab[i] = (zs > 0).astype(float)
        else:
            mu1, mu2 = home * tau / T, away * tau / T
            tab[i] = skellam.sf(-zs, mu1, mu2)   # P(D > -z) = P(z + D > 0)
    return tab


def simulate_game(home, away):
    lam_h, lam_a = home / T * DT, away / T * DT
    gh = rng.poisson(lam_h, size=(NPATH, NSTEP))
    ga = rng.poisson(lam_a, size=(NPATH, NSTEP))
    goals = gh + ga
    z = np.zeros((NPATH, NSTEP + 1), dtype=int)
    z[:, 1:] = np.cumsum(gh - ga, axis=1)
    z = np.clip(z, -ZMAX, ZMAX)
    tab = fair_value_table(home, away)
    m = tab[np.arange(NSTEP + 1)[None, :], z + ZMAX]
    # half-spread at each base time: wide for WIDEN_MIN minutes after any goal
    last_goal = np.full(NPATH, -1e9)
    spread = np.empty((NPATH, NSTEP + 1))
    for i in range(NSTEP + 1):
        t = i * DT
        if i > 0:
            last_goal = np.where(goals[:, i - 1] > 0, t, last_goal)
        spread[:, i] = np.where(t - last_goal < WIDEN_MIN, S0 * WIDEN, S0)
    return m, spread


mA, sA = simulate_game(**GAMES["A"])
mB, sB = simulate_game(**GAMES["B"])
pA, pB = float(mA[0, 0]), float(mB[0, 0])
outA, outB = mA[:, -1], mB[:, -1]


def run_policy(N, every, eta=0.0, frictions=False, x0=None, kind="band"):
    """Simulate a policy rebalancing every `every` base steps.
    kind: 'band' (eta=0 is the bilinear hedge), 'cash' (never trades),
          'static' (buy N of the cheaper leg at t0)."""
    idx = list(range(0, NSTEP, every)) + [NSTEP]     # t_0..t_{K-1}, t_K = T
    K = len(idx) - 1
    if x0 is None:
        x0 = N * pA * pB
    qA = np.zeros(NPATH); qB = np.zeros(NPATH)
    cash = np.full(NPATH, x0, dtype=float)
    L = np.zeros(NPATH)
    volA_wide = np.zeros(NPATH); volA = np.zeros(NPATH)
    hA = hB = eta * N
    for k in range(K):
        i = idx[k]
        a, b = mA[:, i], mB[:, i]
        if kind == "band":
            tA, tB = N * b, N * a
            newA = np.clip(qA, tA - hA, tA + hA)
            newB = np.clip(qB, tB - hB, tB + hB)
        elif kind == "cash":
            newA, newB = qA, qB
        elif kind == "static":
            if k == 0:
                newA = np.full(NPATH, float(N) if pA <= pB else 0.0)
                newB = np.full(NPATH, 0.0 if pA <= pB else float(N))
            else:
                newA, newB = qA, qB
        dA, dB = newA - qA, newB - qB
        cash -= a * dA + b * dB
        if frictions:
            cA = sA[:, i] * np.abs(dA) + KAPPA * dA**2
            cB = sB[:, i] * np.abs(dB) + KAPPA * dB**2
            cash -= cA + cB
            L += cA + cB
            if k >= 1:   # rebalancing trades only (exclude the entry trade at t_0)
                volA += np.abs(dA)
                volA_wide += np.abs(dA) * (sA[:, i] > S0)
        qA, qB = newA, newB
    XT = cash + qA * outA + qB * outB
    E = XT - N * outA * outB
    # sample estimate of the variance floor N^2 sum E[dA^2 dB^2]
    dAs = np.diff(mA[:, idx], axis=1); dBs = np.diff(mB[:, idx], axis=1)
    floor = N**2 * np.mean(np.sum(dAs**2 * dBs**2, axis=1))
    return dict(E=E, L=L, floor=floor, K=K, volA=volA, volA_wide=volA_wide)


res = {"pA": pA, "pB": pB, "PA_PB": pA * pB,
       "P_joint_mc": float(np.mean(outA * outB)),
       "pA_mc": float(outA.mean()), "pB_mc": float(outB.mean()), "npath": NPATH}

# ---------- Experiment 1: frictionless, rebalance every minute ----------
N1 = 1000.0
ev = int(1 / DT)
rows = {}
for name, kw in {"cash": dict(kind="cash"),
                 "static": dict(kind="static", x0=N1 * min(pA, pB)),
                 "bilinear": dict(kind="band", eta=0.0)}.items():
    r = run_policy(N1, ev, **kw)
    E = r["E"] / N1
    rows[name] = dict(capital=float((kw.get("x0", N1 * pA * pB)) / N1),
                      mean=float(E.mean()), se=float(E.std(ddof=1) / np.sqrt(NPATH)),
                      std=float(E.std(ddof=1)), p_short=float(np.mean(E < -1e-9)),
                      q01=float(np.quantile(E, 0.01)), min=float(E.min()))
    if name == "bilinear":
        rows[name]["floor_std"] = float(np.sqrt(r["floor"]) / N1)
res["exp1"] = rows

# ---------- Experiment 2: std of error vs rebalancing interval ----------
exp2 = []
for every in [1, 4, 20, 60, 180, 360]:          # 0.25, 1, 5, 15, 45, 90 minutes
    r = run_policy(N1, every, kind="band", eta=0.0)
    exp2.append(dict(interval=every * DT, K=r["K"],
                     std=float(r["E"].std(ddof=1) / N1),
                     floor_std=float(np.sqrt(r["floor"]) / N1),
                     mean=float(r["E"].mean() / N1)))
res["exp2"] = exp2

# ---------- Experiment 3: band policies with frictions ----------
N3 = 10000.0
ETAS = [0.0, 0.01, 0.02, 0.05, 0.1, 0.2, 0.4]
exp3 = []
for eta in ETAS:
    r = run_policy(N3, ev, eta=eta, frictions=True)
    exp3.append(dict(eta=eta, EL=float(r["L"].mean() / N3),
                     std=float(r["E"].std(ddof=1) / N3),
                     mean=float(r["E"].mean() / N3),
                     wide_share=float(r["volA_wide"].sum() / max(r["volA"].sum(), 1e-12))))
res["exp3"] = exp3

# entry cost of the bilinear hedge
res["entry_over_N"] = float((S0 * (N3 * pB + N3 * pA) + KAPPA * ((N3 * pB)**2 + (N3 * pA)**2)) / N3)

# ---------- Experiment 4: effective price and odds vs stake ----------
MARGIN = 0.05

def static_cost(N):
    # ladders: level size 10 contracts, marginal price p + s + 2*kappa*q, prices < 0.99
    lv = []
    for p in (pA, pB):
        q = 0.0
        while True:
            price = p + S0 + 2 * KAPPA * (q + 5.0)
            if price >= 0.99:
                break
            lv.append(price); q += 10.0
    lv = np.sort(np.array(lv))
    units = N / 10.0
    full = int(min(units, lv.size))
    cost = 10.0 * lv[:full].sum()
    if units > full:
        if full < lv.size:
            cost += (units - full) * 10.0 * lv[full]
        else:
            cost += (N - 10.0 * lv.size)    # cash beyond depth
    return cost

N_GRID = np.array([100, 200, 500, 1000, 2000, 5000, 10000, 20000, 40000, 80000], dtype=float)
table = []   # per N: list of (eta, E[L], Std(E))
for N in N_GRID:
    row = []
    for eta in ETAS:
        r = run_policy(N, ev, eta=eta, frictions=True)
        row.append((eta, float(r["L"].mean()), float(r["E"].std(ddof=1))))
    table.append(row)
gst = np.array([static_cost(N) / N for N in N_GRID])

def gst_of(N):
    return np.interp(np.log(N), np.log(N_GRID), gst)

def solve(S, gfun):
    lo, hi = 1e-6, 100.0
    for _ in range(200):
        mid = 0.5 * (lo + hi)
        if mid * gfun(S * mid) > 1 - MARGIN:
            hi = mid
        else:
            lo = mid
    return 0.5 * (lo + hi)

S_GRID = np.geomspace(150, 20000, 25)
res["exp4"] = dict(N=N_GRID.tolist(), g_st=gst.tolist(), S=S_GRID.tolist(), table=table,
                   odd_static=[solve(S, gst_of) for S in S_GRID],
                   odd_competitive=(1 - MARGIN) / (pA * pB), margin=MARGIN, by_lambda={})
for lam in (1.0, 0.25):
    gdyn, best_eta = [], []
    for N, row in zip(N_GRID, table):
        val, eta = min((EL + lam * sd, eta) for eta, EL, sd in row)
        gdyn.append((N * pA * pB + val) / N); best_eta.append(eta)
    gdyn = np.array(gdyn); g = np.minimum(gdyn, gst)
    gfun = lambda N, g=g: np.interp(np.log(N), np.log(N_GRID), g)
    res["exp4"]["by_lambda"][str(lam)] = dict(
        g_dyn=gdyn.tolist(), best_eta=best_eta,
        g_monotone=bool(np.all(np.diff(g) >= -1e-12)),
        odd=[solve(S, gfun) for S in S_GRID])
    print("lambda", lam, "g_dyn", np.round(gdyn, 4), "eta", best_eta,
          "odds", np.round(res["exp4"]["by_lambda"][str(lam)]["odd"][::6], 3))
print("g_st", np.round(gst, 4), "odd_st", np.round(res["exp4"]["odd_static"][::6], 3))

# share of time leg A's spread is wide (benchmark for crossed liquidity)
res["wide_time_share_A"] = float(np.mean(sA[:, :NSTEP] > S0))
res["wide_time_share_A_minutes"] = float(np.mean(sA[:, 4:NSTEP:4] > S0))
json.dump(res, open(sys.argv[1], "w"), indent=1)
print(json.dumps({k: v for k, v in res.items() if k not in ("exp4",)}, indent=1))
