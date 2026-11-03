"""Hand-written reference implementation, written from requirements.md only.

Used by gen.py to place cases next to the derived thresholds, by the tests to check examples.json, and by oracle.py to
cross-check the oracle (written later, by another model family, from requirements.md alone). Not shown to either arm.
"""
from decimal import ROUND_HALF_UP, Decimal

D = Decimal
CENT = D("0.01")
TIER_RATE = {"standard": D(0), "silver": D("0.03"), "gold": D("0.05")}


def cents(x):
    return x.quantize(CENT, ROUND_HALF_UP)


def volume_rate(quantity):
    if quantity >= 200:
        return D("0.12")
    if quantity >= 50:
        return D("0.08")
    if quantity >= 10:
        return D("0.04")
    return D(0)


def discounted_subtotal(customer_tier, unit_price, quantity):
    """(volume discount, tier discount, discounted subtotal): steps 1-5."""
    subtotal = unit_price * quantity
    volume = cents(subtotal * volume_rate(quantity))
    after_volume = subtotal - volume
    tier = cents(after_volume * TIER_RATE[customer_tier])
    return volume, tier, after_volume - tier


def coupon_discount(coupon, discounted):
    if coupon == "SAVE15" and discounted >= 100:
        return min(cents(discounted * D("0.15")), D(40))
    if coupon == "FLAT25" and discounted >= 200:
        return D(25)
    return D(0)


def price_order(customer_tier, unit_price, quantity, coupon):
    volume, tier, discounted = discounted_subtotal(customer_tier, unit_price, quantity)
    coupon_off = coupon_discount(coupon, discounted)
    merchandise = discounted - coupon_off
    shipping = D(0) if customer_tier == "gold" or merchandise >= 75 else D("6.95")
    return {
        "volume_discount": volume,
        "tier_discount": tier,
        "coupon_discount": coupon_off,
        "shipping": shipping,
        "total": merchandise + shipping,
    }
