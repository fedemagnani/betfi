import json, sys
U = 1_000_000
def payout(s_u, r_ppm, legs):
    rs = s_u * r_ppm // U
    acc = rs
    for m1, f in legs:
        acc = acc * (U - m1) // f
    return rs, acc
def settle(p_u, rt):
    acc = p_u
    for x in rt:
        acc = acc * x // U
    return acc
def floor_ok(p_u, rs_u, mppu):
    return p_u * U >= mppu * rs_u
cases = [
 ("3 legs overlapping at 0.50, full fill", 10_000_000, U, [(50_000,500_000)]*3),
 ("3 legs sequential at 0.50, full fill", 10_000_000, U, [(30_000,500_000)]*3),
 ("1 leg sequential at 0.50", 10_000_000, U, [(30_000,500_000)]),
 ("5 legs overlapping at 0.50", 10_000_000, U, [(50_000,500_000)]*5),
 ("5 legs sequential at 0.50", 10_000_000, U, [(30_000,500_000)]*5),
 ("mixed tiers, odd prices, r = 0.6, 5 USDC", 5_000_000, 600_000, [(50_000,695_000),(50_000,412_345),(30_000,233_001)]),
 ("2 legs, long favourite and short underdog, r = 1", 7_500_000, U, [(30_000,770_000),(30_000,241_000)]),
]
out = {"_doc": "Frozen payout vectors (plan L1 contract C2). Integer micro-units: 1.0 = 1_000_000. "
       "rs_u = floor(stake_u * r_ppm / 1e6); acc = rs_u; for each leg in order: acc = floor(acc * (1e6 - m1_ppm) / f_u); payout_u = acc. "
       "odds_u = floor(payout_u * 1e6 / rs_u). settle: acc = payout_u; for each leg: acc = floor(acc * rtilde_ppm / 1e6). "
       "Floor check (finalize): payout_u * 1e6 >= min_payout_per_unit_u * rs_u. All arithmetic in u128. pricing (1.B) and the program (1.C) must reproduce every number.",
       "payout": [], "settle": [], "floor": []}
for name, s, r, legs in cases:
    rs, p = payout(s, r, legs)
    out["payout"].append({"name": name, "stake_u": s, "r_ppm": r,
        "legs": [{"m1_ppm": m, "f_u": f} for m, f in legs],
        "rs_u": rs, "refund_u": s - rs, "payout_u": p, "odds_u": p * U // rs})
p3 = out["payout"][0]["payout_u"]
for name, rt in [("all won (long)", [U,U,U]), ("one half-void", [U,500_000,U]), ("one lost", [U,0,U]), ("two half-voids", [500_000,500_000,U])]:
    out["settle"].append({"name": name, "payout_u": p3, "rtilde_ppm": rt, "paid_u": settle(p3, rt)})
# floor: case 0 at its exact odds passes, one micro-unit above fails
c = out["payout"][0]
for name, mppu in [("floor equal to achieved odds", c["odds_u"]), ("floor one micro above", c["odds_u"]+1), ("3% slippage floor", c["odds_u"]*970_000//U)]:
    out["floor"].append({"name": name, "payout_u": c["payout_u"], "rs_u": c["rs_u"], "min_payout_per_unit_u": mppu, "ok": floor_ok(c["payout_u"], c["rs_u"], mppu)})
json.dump(out, sys.stdout, indent=2)
