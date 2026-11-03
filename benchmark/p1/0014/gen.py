"""Generate 0014's held-out inputs: every amount where a ranking changes, landed on or beside, then uniform random.

Each product's down payment, monthly payment and 36-month balance is a straight line in the requested amount; equity
falls as balance / amount rises, so equity ranks like the balance reversed. Two products swap places in a ranking
exactly where their lines cross. A crossing at a whole cent is a tie (both
products keep table order) and gets the cent itself and one cent either side; any other crossing gets the cents just
below and just above it. With the range ends that is about three quarters of the 200 cases; the rest are uniform.
Amounts are exact cents inside the stated range (5000.00 to 1500000.00).
"""
import random
from decimal import ROUND_CEILING, ROUND_FLOOR, Decimal as D, localcontext

import reference as ref

LO, HI = D("5000.00"), D("1500000.00")
CENT = D("0.01")


def lines():
    """{ranking: [(slope, intercept) per product]}: down payment, monthly payment and 36-month balance per amount."""
    out = {"down": [], "monthly": [], "equity": []}
    for p in ref.bankrates():
        m, g = 1 + p["points"] / 100, 1 + p["rate"] / 12
        per_unit = p["rate"] / 12 / (1 - g ** -360)  # monthly payment per unit of loan
        keep = g ** 36 - per_unit * (g ** 36 - 1) / p["rate"]  # 36-month balance per unit of loan
        for name, w in (("down", D("0.2")), ("monthly", per_unit), ("equity", keep)):
            out[name].append((w * m, w * p["fee"]))
    return out


def crossings():
    """{amount: tag} for every pairwise crossing inside the range, computed to 50 digits."""
    found = {}
    with localcontext() as ctx:
        ctx.prec = 50
        for name, ls in lines().items():
            for i in range(len(ls)):
                for j in range(i + 1, len(ls)):
                    (ai, bi), (aj, bj) = ls[i], ls[j]
                    if ai != aj and LO <= (x := (bj - bi) / (ai - aj)) <= HI:
                        found.setdefault(x, f"{name}:{i}-{j}")
    return found


def boundary_amounts():
    """[(amount, tag)], sorted and unique, all inside the range."""
    pts = {LO: "range:lo", LO + CENT: "range:lo", HI - CENT: "range:hi", HI: "range:hi"}
    for x, tag in crossings().items():
        lo, hi = x.quantize(CENT, ROUND_FLOOR), x.quantize(CENT, ROUND_CEILING)
        near = [lo - CENT, lo, lo + CENT] if lo == hi else [lo, hi]
        for a in near:
            if LO <= a <= HI:
                pts.setdefault(a, f"{tag}:{'eq' if a == x else '-' if a < x else '+'}")
    return sorted(pts.items())


def generate(seed, n):
    rng = random.Random(seed)
    edges = boundary_amounts()
    if len(edges) > n:
        raise ValueError(f"{len(edges)} boundary amounts don't fit in {n} cases")
    rng.shuffle(edges)
    cases = [(a, [t], "boundary") for a, t in edges[:n]]
    lo, hi = int(LO * 100), int(HI * 100)
    cases += [(D(rng.randint(lo, hi)).scaleb(-2), ["random"], "random") for _ in range(n - len(cases))]
    rng.shuffle(cases)
    return [{"id": i + 1, "requested_amt": a, "tags": t, "kind": k} for i, (a, t, k) in enumerate(cases)]
