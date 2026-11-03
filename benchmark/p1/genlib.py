"""Helpers the programs' gen.py files share (they import it as `genlib`; benchmark/p1 is on sys.path)."""
from decimal import Decimal as D


def disagrees(x):
    """x ends in exactly half a cent after an even cent digit, where half-up and half-even round differently."""
    x *= 100
    return x % 1 == D("0.5") and int(x) % 2 == 0


def bracket(f, target, lo, hi):
    """The values either side of where f (non-decreasing on lo..hi) reaches target: the last below, the first at or
    above. Empty when target is out of reach in the range."""
    if f(hi) < target or f(lo) >= target:
        return []
    while hi - lo > 1:  # f(lo) < target <= f(hi)
        mid = (lo + hi) // 2
        lo, hi = (mid, hi) if f(mid) < target else (lo, mid)
    return [lo, hi]


def round_robin(groups, n, key):
    """Up to n distinct cases (by `key`), popped from the groups in turn, so every kind is in the first n."""
    g = list(groups.values())
    out, seen, k = [], set(), 0
    while len(out) < n and any(g):
        lst = g[k % len(g)]
        k += 1
        if lst and key(case := lst.pop()) not in seen:
            seen.add(key(case))
            out.append(case)
    return out
