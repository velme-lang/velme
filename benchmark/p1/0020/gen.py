"""Generate 0020's held-out inputs: ~60% on and next to the rules' thresholds, ~40% uniform random.

Thresholds from requirements.md: age 18, 45 and 60; years of service 15 and 30; plus each input's range ends. Every
case is whole numbers inside the stated ranges (age 16..80, years of service 0..60).
"""
import itertools
import random
from decimal import Decimal as D

AGE_MIN, AGE_MAX, YEARS_MIN, YEARS_MAX = 16, 80, 0, 60
AGE_EDGES = [18, 45, 60]
YEARS_EDGES = [15, 30]


def near(edges, lo, hi):
    """Each edge and the whole numbers either side of it, plus the range ends, all inside [lo, hi]."""
    pts = {lo, hi} | {e + d for e in edges for d in (-1, 0, 1)}
    return sorted(p for p in pts if lo <= p <= hi)


AGE_PTS = near(AGE_EDGES, AGE_MIN, AGE_MAX)
YEARS_PTS = near(YEARS_EDGES, YEARS_MIN, YEARS_MAX)


def boundary_cases(rng, n):
    out = [(a, y, [f"age:{a}", f"years:{y}"]) for a, y in itertools.product(AGE_PTS, YEARS_PTS)]
    k = 0
    while len(out) < n:  # one input on an edge, the other anywhere in its range
        if k % 2:
            a = rng.choice(AGE_PTS)
            out.append((a, rng.randint(YEARS_MIN, YEARS_MAX), [f"age:{a}"]))
        else:
            y = rng.choice(YEARS_PTS)
            out.append((rng.randint(AGE_MIN, AGE_MAX), y, [f"years:{y}"]))
        k += 1
    rng.shuffle(out)
    return out[:n]


def generate(seed, n):
    rng = random.Random(seed)
    nb = round(n * 0.6)
    cases = [(a, y, t, "boundary") for a, y, t in boundary_cases(rng, nb)]
    cases += [(rng.randint(AGE_MIN, AGE_MAX), rng.randint(YEARS_MIN, YEARS_MAX), ["random"], "random")
              for _ in range(n - nb)]
    rng.shuffle(cases)
    return [{"id": i + 1, "age": D(a), "years_of_service": D(y), "tags": t, "kind": k}
            for i, (a, y, t, k) in enumerate(cases)]
