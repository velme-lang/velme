"""Generate refund-policy's held-out inputs: ~60% on and next to the rules' thresholds, ~40% uniform random.

Thresholds from requirements.md, each hit on, one day below and one day above: the return window's last day and the
grace period's last day for every category and membership (nine windows); the 180-day warranty for a defective item;
the 75.00 restocking fee cap (the cent prices either side of where the rounded fee reaches 75.00 and where it passes
it, searched cent by cent with reference.py) for each category and membership with a fee; fees and 50% store credits
whose unrounded value is exactly half a cent after an even cent (half-up rounds up where half-even rounds down); a fee
rate of 0 on an opened item (`full`, not `partial`); plus each input's range ends. Every case is inside the stated
ranges: days 0..365, price 1.00..5000.00 in whole cents.
"""
import random
from decimal import Decimal as D

import genlib
import reference
from genlib import disagrees

CATEGORIES = ["electronics", "apparel", "furniture"]
MEMBERSHIPS = ["none", "plus", "premier"]
CONDITIONS = ["unopened", "opened", "used"]
DAYS_MAX = 365
PRICE_MIN, PRICE_MAX = 100, 500000  # in cents


def price(c):
    return D(c).scaleb(-2)


def bracket(f, target):
    return genlib.bracket(f, target, PRICE_MIN, PRICE_MAX)


def tie(rng, rate, top=PRICE_MAX):
    """A cent price up to `top` whose price × rate is a tie half-up and half-even round differently; None if none
    near."""
    start = rng.randint(PRICE_MIN, top - 400)
    return next((c for c in range(start, min(start + 400, top + 1)) if disagrees(price(c) * rate)), None)


def groups(rng):
    """Lists of (category, membership, days, condition, defective, cents, tags), one list per kind of threshold."""
    g = {k: [] for k in ("window", "grace", "warranty", "cap", "tie", "zero_fee", "ends")}
    any_price = lambda: rng.randint(PRICE_MIN, PRICE_MAX)
    for cat in CATEGORIES:
        for mem in MEMBERSHIPS:
            w = reference.window_days(cat, mem)
            for d in (w - 1, w, w + 1):
                # fix round 1, finding 4: used apparel gets none on both sides of the window, so it decides nothing
                conds = ["unopened", "opened"] if cat == "apparel" else CONDITIONS
                g["window"].append((cat, mem, d, rng.choice(conds), False, any_price(), [f"window:{d - w}"]))
            for d in (w + 29, w + 30, w + 31):
                g["grace"].append((cat, mem, d, rng.choice(["unopened", "opened"]), False, any_price(),
                                   [f"grace:{d - w - 30}"]))
            if reference.fee_rate(cat, mem) > 0:
                fee = lambda c, cat=cat, mem=mem: reference.cents(price(c) * reference.fee_rate(cat, mem))
                for target in (D("75.00"), D("75.01")):
                    for c in bracket(fee, target):
                        g["cap"].append((cat, mem, rng.randint(0, w), "opened", False, c, ["cap"]))
            else:
                g["zero_fee"].append((cat, mem, rng.randint(0, w), "opened", False, any_price(), ["zero_fee"]))
    for d in (179, 180, 181):
        for _ in range(3):
            g["warranty"].append((rng.choice(CATEGORIES), rng.choice(MEMBERSHIPS), d, rng.choice(CONDITIONS), True,
                                  any_price(), [f"warranty:{d}"]))
    while len(g["tie"]) < 40:  # the largest group, so every window and grace case fits in 120
        cat, mem = rng.choice(CATEGORIES), rng.choice(MEMBERSHIPS)
        w = reference.window_days(cat, mem)
        opened = reference.fee_rate(cat, mem) > 0 and rng.random() < 0.5
        if not opened and cat == "apparel":
            continue  # used apparel gets nothing, so its credit cannot tie
        rate = reference.fee_rate(cat, mem) if opened else reference.USED_CREDIT
        # fix round 1, finding 1: an opened item's fee ties only below the 75.00 cap, where the rounding still shows
        top = int(reference.FEE_CAP / rate * 100) - 1 if opened else PRICE_MAX
        if (c := tie(rng, rate, top)) is not None:
            g["tie"].append((cat, mem, rng.randint(0, w), "opened" if opened else "used", False, c, ["tie"]))
    for d in (0, DAYS_MAX):
        for c in (PRICE_MIN, PRICE_MAX):
            g["ends"].append((rng.choice(CATEGORIES), rng.choice(MEMBERSHIPS), d, rng.choice(CONDITIONS),
                              rng.random() < 0.5, c, ["ends"]))
    for lst in g.values():
        rng.shuffle(lst)
    return g


def boundary_cases(rng, n):
    """Round-robin over the threshold kinds, so every kind is in the first n."""
    return genlib.round_robin(groups(rng), n, lambda c: c[:6])


def generate(seed, n):
    rng = random.Random(seed)
    nb = round(n * 0.6)
    cases = [(*c, "boundary") for c in boundary_cases(rng, nb)]
    cases += [(rng.choice(CATEGORIES), rng.choice(MEMBERSHIPS), rng.randint(0, DAYS_MAX), rng.choice(CONDITIONS),
               rng.random() < 0.5, rng.randint(PRICE_MIN, PRICE_MAX), ["random"], "random") for _ in range(n - nb)]
    rng.shuffle(cases)
    return [{"id": i + 1, "category": cat, "membership": mem, "days_since_purchase": D(d), "condition": cond,
             "defective": dft, "price": price(c), "tags": tags, "kind": k}
            for i, (cat, mem, d, cond, dft, c, tags, k) in enumerate(cases)]
