"""Generate order-pricing's held-out inputs: ~60% on and next to the rules' thresholds, ~40% uniform random.

Thresholds from requirements.md: quantity 10, 50 and 200 (volume bands); discounted subtotal 100.00 (SAVE15) and
200.00 (FLAT25); the SAVE15 cap (15% of the discounted subtotal reaching 40.00); merchandise total 75.00 (shipping);
discounts whose third decimal is exactly 5 after an even cent (half-up ties where half-even would round down); plus each input's range ends. The derived thresholds are
hit by searching the unit price, cent by cent, with reference.py. Every case is inside the stated ranges: unit price
0.50..500.00 in whole cents, quantity 1..1000.
"""
import random
from decimal import Decimal as D

import reference

TIERS = ["standard", "silver", "gold"]
COUPONS = ["NONE", "SAVE15", "FLAT25"]
PRICE_MIN, PRICE_MAX = 50, 50000  # in cents
QTY_MIN, QTY_MAX = 1, 1000
QTY_EDGES = [10, 50, 200]
QTY_PTS = sorted({QTY_MIN, QTY_MAX} | {e + d for e in QTY_EDGES for d in (-1, 0, 1)})


def price(c):
    return D(c).scaleb(-2)


def discounted(tier, c, q):
    return reference.discounted_subtotal(tier, price(c), q)[2]


def merchandise(tier, c, q, coupon):
    d = discounted(tier, c, q)
    return d - reference.coupon_discount(coupon, d)


def bracket(f, target):
    """The cent prices either side of where f (non-decreasing in the price) reaches target: the last below, the first
    at or above. Empty when target is out of reach in the price range."""
    if f(PRICE_MAX) < target or f(PRICE_MIN) >= target:
        return []
    lo, hi = PRICE_MIN, PRICE_MAX  # f(lo) < target <= f(hi)
    while hi - lo > 1:
        mid = (lo + hi) // 2
        lo, hi = (mid, hi) if f(mid) < target else (lo, mid)
    return [lo, hi]


def disagrees(x):
    """x ends in exactly half a cent after an even cent digit, where half-up and half-even round differently."""
    x *= 100
    return x % 1 == D("0.5") and int(x) % 2 == 0


def tie(rng, tier, q, coupon):
    """A cent price whose tier or coupon discount, before rounding, is a tie half-up and half-even round differently;
    None if none near."""
    start = rng.randint(PRICE_MIN, PRICE_MAX - 400)
    for c in range(start, start + 400):
        volume, _, d = reference.discounted_subtotal(tier, price(c), q)
        after_volume = price(c) * q - volume
        raw = [after_volume * reference.TIER_RATE[tier]]
        if coupon == "SAVE15" and d >= 100:
            raw.append(d * D("0.15"))
        if any(disagrees(x) for x in raw):
            return c
    return None


def groups(rng):
    """Lists of (tier, cents, quantity, coupon, tags), one list per kind of threshold."""
    g = {"qty": [], "save15": [], "flat25": [], "cap": [], "ship": [], "tie": [], "ends": []}
    for q in QTY_PTS:
        for tier in TIERS:
            g["qty"].append((tier, rng.randint(PRICE_MIN, PRICE_MAX), q, rng.choice(COUPONS), [f"qty:{q}"]))
    for tier in TIERS:
        for q in [1, 2, 3] + rng.sample(range(4, 300), 3):  # small quantities land on the cent
            # SAVE15's 40.00 cap: 266.64 is where the rounded 15% first reaches 40.00, 266.67 where the unrounded 15%
            # first exceeds 40.00, 266.70 where the rounded 15% first exceeds it.
            for name, coupon, target in (("save15", "SAVE15", 100), ("flat25", "FLAT25", 200),
                                         ("cap", "SAVE15", D("266.64")), ("cap", "SAVE15", D("266.67")),
                                         ("cap", "SAVE15", D("266.70"))):
                for c in bracket(lambda c, t=tier, q=q: discounted(t, c, q), target):
                    g[name].append((tier, c, q, coupon, [f"{name}:{q}"]))
            if tier != "gold":
                coupon = rng.choice(COUPONS)
                for c in bracket(lambda c, t=tier, q=q, cp=coupon: merchandise(t, c, q, cp), 75):
                    g["ship"].append((tier, c, q, coupon, [f"ship:{q}"]))
    while len(g["tie"]) < 30:
        tier, q, coupon = rng.choice(["silver", "gold"]), rng.choice(QTY_PTS + [rng.randint(1, 1000)]), rng.choice(COUPONS)
        if (c := tie(rng, tier, q, coupon)) is not None:
            g["tie"].append((tier, c, q, coupon, ["tie"]))
    for c in (PRICE_MIN, PRICE_MAX):
        for q in (QTY_MIN, QTY_MAX):
            g["ends"].append((rng.choice(TIERS), c, q, rng.choice(COUPONS), ["ends"]))
    for lst in g.values():
        rng.shuffle(lst)
        lst.sort(key=lambda c: c[2] == 1)  # popped first: at quantity 1 a cent price lands exactly on a threshold
    return g


def boundary_cases(rng, n):
    """Round-robin over the threshold kinds, so every kind is in the first n."""
    g = list(groups(rng).values())
    out, seen, k = [], set(), 0
    while len(out) < n and any(g):
        lst = g[k % len(g)]
        k += 1
        if lst and (case := lst.pop())[:4] not in seen:
            seen.add(case[:4])
            out.append(case)
    return out


def generate(seed, n):
    rng = random.Random(seed)
    nb = round(n * 0.6)
    cases = [(*c, "boundary") for c in boundary_cases(rng, nb)]
    cases += [(rng.choice(TIERS), rng.randint(PRICE_MIN, PRICE_MAX), rng.randint(QTY_MIN, QTY_MAX), rng.choice(COUPONS),
               ["random"], "random") for _ in range(n - nb)]
    rng.shuffle(cases)
    return [{"id": i + 1, "customer_tier": t, "unit_price": price(c), "quantity": D(q), "coupon": cp, "tags": tags, "kind": k}
            for i, (t, c, q, cp, tags, k) in enumerate(cases)]
