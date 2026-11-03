"""Generate insurance-quote's held-out inputs: ~60% on and next to the rules' thresholds, ~40% uniform random.

Thresholds from requirements.md: age 21 (young-driver factor), 25 and 65 (age bands); claims 1 and 3 (claims
factor); mileage 20000 and 35000 (loadings); the 300.00 minimum and the 3500.00 cap (the premiums just under and
over each); discounts and monthly instalments whose third decimal is exactly 5 after an even cent (ties half-up and
half-even round differently; a loaded premium never makes one); plus each input's range
ends. Every case is whole numbers inside the stated ranges (age 17..90, claims 0..5, mileage 0..60000).
"""
import itertools
import random
from decimal import Decimal as D

import reference

GROUPS = ["A", "B", "C", "D"]
AGE_MIN, AGE_MAX, CLAIMS_MAX, MILES_MAX = 17, 90, 5, 60000
AGE_PTS = sorted({AGE_MIN, AGE_MAX} | {e + d for e in (21, 25, 65) for d in (-1, 0, 1)})
CLAIMS_PTS = list(range(CLAIMS_MAX + 1))
MILES_PTS = sorted({0, MILES_MAX} | {e + d for e in (20000, 35000) for d in (-1, 0, 1)})


def grid():
    return list(itertools.product(AGE_PTS, GROUPS, CLAIMS_PTS, MILES_PTS, (False, True)))


def disagrees(x):
    """x ends in exactly half a cent after an even cent digit, where half-up and half-even round differently."""
    x *= 100
    return x % 1 == D("0.5") and int(x) % 2 == 0


def ties(case):
    """Which roundings to the cent of this case are ties half-up and half-even round differently: `loaded` (step 2),
    `discount` (step 3), `instalment` (step 5, on the annual premium after the minimum and cap)."""
    age, group, claims, miles, tel = case
    raw = reference.base_rate(age, group) * reference.claims_factor(claims) + reference.mileage_loading(miles)
    if age <= 20:
        raw *= D("1.15")
    loaded = reference.cents(raw)
    keep = (D("0.90") if tel else D(1)) * (D("0.85") if claims == 0 else D(1))
    annual = reference.quote(*case)["annual_premium"]
    steps = {"loaded": raw, "discount": loaded * (1 - keep), "instalment": annual * D("1.05") / 12}
    return [name for name, x in steps.items() if disagrees(x)]


def limit_cases(cases, limit, k):
    """The k cases just under and the k just over a limit, by premium before the minimum and cap."""
    net = {c: reference.before_limits(*c)[3] for c in cases}
    under = sorted((c for c in cases if net[c] < limit), key=lambda c: limit - net[c])[:k]
    over = sorted((c for c in cases if net[c] >= limit), key=lambda c: net[c] - limit)[:k]
    return under + over


def groups(rng):
    """Lists of (age, group, claims, mileage, telematics, tags), one list per kind of threshold."""
    cases = grid()
    g = {
        "age": [(a, rng.choice(GROUPS), rng.randint(0, CLAIMS_MAX), rng.randint(0, MILES_MAX), rng.random() < 0.5)
                for a in AGE_PTS for _ in range(2)],
        "claims": [(rng.randint(AGE_MIN, AGE_MAX), rng.choice(GROUPS), c, rng.randint(0, MILES_MAX), rng.random() < 0.5)
                   for c in CLAIMS_PTS for _ in range(2)],
        "miles": [(rng.randint(AGE_MIN, AGE_MAX), rng.choice(GROUPS), rng.randint(0, CLAIMS_MAX), m, rng.random() < 0.5)
                  for m in MILES_PTS for _ in range(2)],
        "minimum": limit_cases(cases, reference.MINIMUM, 8),
        "cap": limit_cases(cases, reference.CAP, 8),
        "tie": rng.sample([c for c in cases if "discount" in ties(c)], 8)
               + rng.sample([c for c in cases if "instalment" in ties(c)], 8),
        "grid": rng.sample(cases, 60),
    }
    for name, lst in g.items():
        rng.shuffle(lst)
        g[name] = [(*c, [name]) for c in lst]
    return g


def boundary_cases(rng, n):
    """Round-robin over the threshold kinds, so every kind is in the first n."""
    g = list(groups(rng).values())
    out, seen, k = [], set(), 0
    while len(out) < n and any(g):
        lst = g[k % len(g)]
        k += 1
        if lst and (case := lst.pop())[:5] not in seen:
            seen.add(case[:5])
            out.append(case)
    return out


def generate(seed, n):
    rng = random.Random(seed)
    nb = round(n * 0.6)
    cases = [(*c, "boundary") for c in boundary_cases(rng, nb)]
    cases += [(rng.randint(AGE_MIN, AGE_MAX), rng.choice(GROUPS), rng.randint(0, CLAIMS_MAX), rng.randint(0, MILES_MAX),
               rng.random() < 0.5, ["random"], "random") for _ in range(n - nb)]
    rng.shuffle(cases)
    return [{"id": i + 1, "driver_age": D(a), "vehicle_group": grp, "claims_last_5_years": D(c), "annual_mileage": D(m),
             "has_telematics": t, "tags": tags, "kind": k}
            for i, (a, grp, c, m, t, tags, k) in enumerate(cases)]
