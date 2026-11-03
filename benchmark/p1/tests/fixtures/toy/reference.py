"""Toy reference: agrees with the toy oracle except at 25, so the oracle's cross-check has one difference to report."""
from decimal import Decimal


def decide(value):
    value = Decimal(value)
    return {"doubled": value * 2 + (1 if value == 25 else 0), "half": value / 2}
