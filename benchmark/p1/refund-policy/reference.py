"""Hand-written reference implementation, written from requirements.md only.

Used by gen.py to place cases next to the derived thresholds, by the tests to check examples.json, and by oracle.py to
cross-check the oracle (written later, by another model family, from requirements.md alone). Not shown to either arm.
"""
from decimal import ROUND_HALF_UP, Decimal

D = Decimal
CENT = D("0.01")
WINDOW = {"electronics": 30, "apparel": 60, "furniture": 14}
EXTENSION = {"none": 0, "plus": 15, "premier": 30}
GRACE_DAYS = 30
WARRANTY_DAYS = 180
FEE_RATE = {"electronics": D("0.15"), "apparel": D(0), "furniture": D("0.20")}
FEE_CAP = D("75.00")
USED_CREDIT = D("0.50")
LATE_CREDIT = D("0.80")


def cents(x):
    return x.quantize(CENT, ROUND_HALF_UP)


def window_days(category, membership):
    return WINDOW[category] + EXTENSION[membership]


def fee_rate(category, membership):
    rate = FEE_RATE[category]
    return {"none": rate, "plus": rate / 2, "premier": D(0)}[membership]


def restocking_fee(category, membership, price):
    return min(cents(price * fee_rate(category, membership)), FEE_CAP)


def refund(category, membership, days_since_purchase, condition, defective, price):
    window = window_days(category, membership)
    fee = D(0)
    if defective and days_since_purchase <= WARRANTY_DAYS:
        kind, amount = "full", price
    elif days_since_purchase <= window:
        if condition == "unopened":
            kind, amount = "full", price
        elif condition == "opened":
            fee = restocking_fee(category, membership, price)
            kind, amount = ("full" if fee == 0 else "partial"), price - fee
        elif category == "apparel":
            kind, amount = "none", D(0)
        else:
            kind, amount = "store_credit", cents(price * USED_CREDIT)
    elif days_since_purchase <= window + GRACE_DAYS and condition != "used":
        kind, amount = "store_credit", cents(price * LATE_CREDIT)
    else:
        kind, amount = "none", D(0)
    return {"refund_type": kind, "amount": amount, "restocking_fee": fee, "window_days": D(window)}
