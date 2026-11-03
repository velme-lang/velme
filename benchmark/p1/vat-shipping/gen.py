"""Generate vat-shipping's held-out inputs: ~60% on and next to the rules' thresholds, ~40% uniform random.

Thresholds from requirements.md: weight 0.500, 2.000 and 10.000 kg (bands), net amount 50.00 (home free shipping) and
150.00 (eu free shipping), goods VAT whose third decimal is exactly 5 (half-to-even ties, both where half-even and
half-up agree and where they differ), every shipping price of every region with each VAT category (some shipping VAT
amounts tie), plus each input's range ends. Every case is inside the stated ranges: net amount 0.01..5000.00 in whole
cents, weight 0.001..30.000 kg in whole grams.
"""
import random
from decimal import Decimal as D

import genlib
import reference

REGIONS = ["home", "eu", "uk", "world"]
CATEGORIES = ["standard", "books", "food"]
NET_MIN, NET_MAX = 1, 500000  # cents
W_MIN, W_MAX = 1, 30000  # grams
W_PTS = sorted({W_MIN, W_MAX} | {e + d for e in (500, 2000, 10000) for d in (-1, 0, 1)} | {29999})
NET_PTS = sorted({NET_MIN, NET_MAX} | {e + d for e in (5000, 15000) for d in (-1, 0, 1)})


def net(c):
    return D(c).scaleb(-2)


def kg(g):
    return D(g).scaleb(-3)


def below_free(rng, region):
    """A net amount (cents) that doesn't get free shipping in this region."""
    top = {"home": 4999, "eu": 14999}.get(region, NET_MAX)
    return rng.randint(NET_MIN, top)


def goods_tie(rng, region, category, differs):
    """A net amount whose goods VAT ends in exactly half a cent; `differs`: one where half-up would round otherwise."""
    rate = reference.VAT[region][category]
    start = rng.randint(NET_MIN, NET_MAX - 1000)
    for c in range(start, start + 1000):
        x = net(c) * rate * 100  # in cents
        if x % 1 == D("0.5") and (int(x) % 2 == 0) == differs:
            return c
    return None


def groups(rng):
    """Lists of (region, category, net cents, grams, tags), one list per kind of threshold."""
    g = {"weight": [], "free": [], "tie": [], "ship_vat": [], "ends": []}
    for region in REGIONS:
        for w in W_PTS:
            g["weight"].append((region, rng.choice(CATEGORIES), below_free(rng, region), w, [f"weight:{w}"]))
        for category in CATEGORIES:
            for w in (250, 1000, 5000, 20000):  # each band's shipping price, so each shipping VAT amount
                g["ship_vat"].append((region, category, below_free(rng, region), w + rng.randint(-200, 200), ["ship_vat"]))
            if region in ("home", "eu"):
                for c in NET_PTS:
                    g["free"].append((region, category, c, rng.choice(W_PTS + [rng.randint(W_MIN, W_MAX)]), [f"net:{c}"]))
            for differs in (True, False):
                if (c := goods_tie(rng, region, category, differs)) is not None:
                    g["tie"].append((region, category, c, rng.randint(W_MIN, W_MAX), [f"tie:{'even' if differs else 'up'}"]))
    for c in (NET_MIN, NET_MAX):
        for w in (W_MIN, W_MAX):
            g["ends"].append((rng.choice(REGIONS), rng.choice(CATEGORIES), c, w, ["ends"]))
    for lst in g.values():
        rng.shuffle(lst)
    return g


def boundary_cases(rng, n):
    """Round-robin over the threshold kinds, so every kind is in the first n."""
    return genlib.round_robin(groups(rng), n, lambda c: c[:4])


def generate(seed, n):
    rng = random.Random(seed)
    nb = round(n * 0.6)
    cases = [(*c, "boundary") for c in boundary_cases(rng, nb)]
    cases += [(rng.choice(REGIONS), rng.choice(CATEGORIES), rng.randint(NET_MIN, NET_MAX), rng.randint(W_MIN, W_MAX),
               ["random"], "random") for _ in range(n - nb)]
    rng.shuffle(cases)
    return [{"id": i + 1, "region": r, "category": cat, "net_amount": net(c), "weight_kg": kg(w), "tags": tags, "kind": k}
            for i, (r, cat, c, w, tags, k) in enumerate(cases)]
