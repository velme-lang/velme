from decimal import Decimal


def decide(case):
    value = Decimal(case["value"])
    return {"tripled": value * 3, "tenth": value / 10}
