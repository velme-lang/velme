"""Toy Python oracle: double and halve the input, exactly."""
from decimal import Decimal


def decide(case):
    value = Decimal(case["value"])
    return {"doubled": value * 2, "half": value / 2}
