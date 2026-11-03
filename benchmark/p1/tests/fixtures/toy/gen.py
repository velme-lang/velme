"""Toy generator: a handful of values, with the case kind set directly (not through 0087-style tags)."""
VALUES = [1, 9, 10, 25]


def generate(seed, n):
    return [{"id": i + 1, "value": v, "kind": "random" if v == 25 else "boundary"} for i, v in enumerate(VALUES[:n])]
