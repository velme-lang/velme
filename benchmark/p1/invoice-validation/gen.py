"""Generate invoice-validation's held-out inputs: ~60% on and next to the rules' thresholds, ~40% uniform random.

Thresholds from requirements.md, each hit on, one unit below and one unit above: line count 1 and 200; received day
- invoice day at 0 (future dated) and 180 (stale); payment terms 60; due day = received day (overdue); declared tax -
computed tax at -0.02 and +0.02 (tolerance); gross total 200000.00 (over limit) and the approver limits 5000.00 and
50000.00 (manual approval), the gross ones hit by searching the net amount cent by cent with reference.py; computed
taxes whose unrounded value is exactly half a cent after an even cent (half-even rounds down where half-up rounds up),
with the declared tax at that computed tax and at the tolerance's edges, so a half-up tax also flips TAX_MISMATCH;
plus each input's range ends. A boundary case starts from an accepted invoice and moves one input onto its threshold,
so the threshold decides the case. Random cases draw every input uniformly from its range, except that half of them
draw the declared tax within 1.00 of the computed tax (else almost every random case would fail TAX_MISMATCH). Every
case is inside the stated ranges.
"""
import random
from decimal import Decimal as D

import reference

REGIONS = ["DE", "NL", "UK", "CH"]
ROLES = ["clerk", "manager", "director"]
NET_MIN, NET_MAX = 1, 25000000  # in cents
TAX_MAX = 6000000  # in cents
LINES_MAX, DAY_MIN, DAY_MAX, TERMS_MAX = 500, 1, 730, 120
FIELDS = ["region", "net_amount", "declared_tax", "line_count", "invoice_day", "received_day", "payment_terms_days",
          "approver_role"]


def money(c):
    return D(c).scaleb(-2)


def tax_cents(region, net):
    return int(reference.computed_tax(region, money(net)) * 100)


def gross(region, net):
    return net + tax_cents(region, net)


def disagrees(region, net):
    """net × rate is exactly half a cent after an even cent, where half-up and half-even round differently."""
    x = money(net) * reference.TAX_RATE[region] * 100
    return x % 1 == D("0.5") and int(x) % 2 == 0


def accepted(rng, region=None, net=None, role=None):
    """An accepted invoice as a dict of whole-number inputs (money in cents), net below the 200000.00 limit."""
    region = region or rng.choice(REGIONS)
    net = rng.randint(NET_MIN, 15000000) if net is None else net
    terms = rng.randint(0, 60)
    inv = rng.randint(DAY_MIN, DAY_MAX - 60)
    return {"region": region, "net": net, "tax": tax_cents(region, net), "lines": rng.randint(1, 200), "inv": inv,
            "rec": inv + rng.randint(0, terms), "terms": terms, "role": role or rng.choice(ROLES)}


def bracket(f, target):
    """The net cents either side of where f (non-decreasing in the net) reaches target: the last below, the first at
    or above."""
    lo, hi = NET_MIN, NET_MAX
    if f(hi) < target or f(lo) >= target:
        return []
    while hi - lo > 1:
        mid = (lo + hi) // 2
        lo, hi = (mid, hi) if f(mid) < target else (lo, mid)
    return [lo, hi]


def groups(rng):
    """Lists of (case dict, tags), one list per kind of threshold."""
    g = {k: [] for k in ("lines", "future", "stale", "terms", "overdue", "tolerance", "limit", "approval", "tie", "ends")}
    for n in (0, 1, 2, 199, 200, 201, LINES_MAX):
        for _ in range(2):
            g["lines"].append(({**accepted(rng), "lines": n}, [f"lines:{n}"]))
    for gap in (-1, 0, 1):
        for _ in range(3):
            c = accepted(rng)
            c["inv"] = rng.randint(DAY_MIN + 1, DAY_MAX - 1)
            g["future"].append(({**c, "rec": c["inv"] + gap, "terms": rng.randint(1, 60)}, [f"future:{gap}"]))
    for age in (179, 180, 181):
        for _ in range(3):
            c = accepted(rng)
            inv = rng.randint(DAY_MIN, DAY_MAX - age)
            # terms are at most 120, so a stale invoice is also overdue: STALE alone flips in the reasons
            g["stale"].append(({**c, "inv": inv, "rec": inv + age, "terms": rng.randint(0, TERMS_MAX)}, [f"stale:{age}"]))
    for t in (0, 59, 60, 61, TERMS_MAX):
        for _ in range(2):
            c = accepted(rng)
            g["terms"].append(({**c, "terms": t, "rec": c["inv"] + rng.randint(0, min(t, 60))}, [f"terms:{t}"]))
    for late in (-1, 0, 1):
        for _ in range(3):
            c = accepted(rng)
            # fix round 1, finding 3: received a day before due needs terms >= 1, or it is future dated (or day 0)
            terms = rng.randint(1 if late < 0 else 0, 60)
            inv = rng.randint(DAY_MIN, DAY_MAX - terms - 1)
            g["overdue"].append(({**c, "inv": inv, "terms": terms, "rec": inv + terms + late}, [f"overdue:{late}"]))
    for off in (-3, -2, -1, 1, 2, 3):
        for _ in range(2):
            c = accepted(rng)
            if 0 <= c["tax"] + off <= TAX_MAX:
                g["tolerance"].append(({**c, "tax": c["tax"] + off}, [f"tolerance:{off}"]))
    for region in REGIONS:
        for net in bracket(lambda n, r=region: gross(r, n), 20000000) + bracket(lambda n, r=region: gross(r, n), 20000001):
            g["limit"].append((accepted(rng, region, net), ["limit"]))
        for role, limit in (("clerk", 500000), ("manager", 5000000)):
            for target in (limit, limit + 1):
                for net in bracket(lambda n, r=region: gross(r, n), target):
                    g["approval"].append((accepted(rng, region, net, role), [f"approval:{role}"]))
    while len(g["tie"]) < 30:
        region = rng.choice(REGIONS)
        start = rng.randint(NET_MIN, NET_MAX - 2000)
        net = next((n for n in range(start, start + 2000) if disagrees(region, n)), None)
        if net is not None:
            c = accepted(rng, region, net)
            off = rng.choice([0, 0, -2, 2, -3, 3])  # half-up computes one cent more: it flips +3 and -2
            if 0 <= c["tax"] + off <= TAX_MAX:
                g["tie"].append(({**c, "tax": c["tax"] + off}, ["tie"]))
    for net in (NET_MIN, NET_MAX):
        c = accepted(rng, net=net)
        g["ends"].append((c, ["ends"]))
        g["ends"].append(({**c, "tax": TAX_MAX if net == NET_MIN else 0}, ["ends"]))
    for inv, rec in ((DAY_MIN, DAY_MIN), (DAY_MAX, DAY_MAX), (DAY_MIN, DAY_MAX), (DAY_MAX, DAY_MIN)):
        g["ends"].append(({**accepted(rng), "inv": inv, "rec": rec}, ["ends"]))
    for lst in g.values():
        rng.shuffle(lst)
    return g


def key(c):
    return tuple(c[k] for k in ("region", "net", "tax", "lines", "inv", "rec", "terms", "role"))


def boundary_cases(rng, n):
    """Round-robin over the threshold kinds, so every kind is in the first n."""
    g = list(groups(rng).values())
    out, seen, k = [], set(), 0
    while len(out) < n and any(g):
        lst = g[k % len(g)]
        k += 1
        if lst and key((case := lst.pop())[0]) not in seen:
            seen.add(key(case[0]))
            out.append(case)
    return out


def random_case(rng):
    region, net = rng.choice(REGIONS), rng.randint(NET_MIN, NET_MAX)
    if rng.random() < 0.5:
        tax = min(max(tax_cents(region, net) + rng.randint(-100, 100), 0), TAX_MAX)
    else:
        tax = rng.randint(0, TAX_MAX)
    return {"region": region, "net": net, "tax": tax, "lines": rng.randint(0, LINES_MAX),
            "inv": rng.randint(DAY_MIN, DAY_MAX), "rec": rng.randint(DAY_MIN, DAY_MAX),
            "terms": rng.randint(0, TERMS_MAX), "role": rng.choice(ROLES)}


def generate(seed, n):
    rng = random.Random(seed)
    nb = round(n * 0.6)
    cases = [(c, tags, "boundary") for c, tags in boundary_cases(rng, nb)]
    cases += [(random_case(rng), ["random"], "random") for _ in range(n - nb)]
    rng.shuffle(cases)
    return [{"id": i + 1, "region": c["region"], "net_amount": money(c["net"]), "declared_tax": money(c["tax"]),
             "line_count": D(c["lines"]), "invoice_day": D(c["inv"]), "received_day": D(c["rec"]),
             "payment_terms_days": D(c["terms"]), "approver_role": c["role"], "tags": tags, "kind": k}
            for i, (c, tags, k) in enumerate(cases)]
