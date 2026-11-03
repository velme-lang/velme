"""Independent Python oracle for order-pricing, derived solely from requirements.md."""
from decimal import Decimal, ROUND_HALF_UP

# requirements.md lines 20-21: "Rounded to the cent" = two decimal places, half up.
CENT = Decimal("0.01")


def round_cent(amount: Decimal) -> Decimal:
    return amount.quantize(CENT, rounding=ROUND_HALF_UP)


# requirements.md lines 26-27: Tier rate: standard 0%, silver 3%, gold 5%.
TIER_RATES = {
    "standard": Decimal("0"),
    "silver": Decimal("0.03"),
    "gold": Decimal("0.05"),
}


def _volume_rate(quantity: Decimal) -> Decimal:
    # requirements.md lines 24-25: Volume rate by quantity (each range includes both ends).
    if quantity <= Decimal(9):
        return Decimal("0")
    if quantity <= Decimal(49):
        return Decimal("0.04")
    if quantity <= Decimal(199):
        return Decimal("0.08")
    return Decimal("0.12")


def decide(case: dict) -> dict:
    customer_tier = case["customer_tier"]
    unit_price = Decimal(case["unit_price"])
    quantity = Decimal(case["quantity"])
    coupon = case["coupon"]

    # requirements.md line 23: Step 1. List subtotal = unit price * quantity.
    list_subtotal = unit_price * quantity

    # requirements.md lines 24-25: Step 2. Volume discount = list subtotal * volume rate, rounded to the cent.
    volume_discount = round_cent(list_subtotal * _volume_rate(quantity))

    # requirements.md lines 26-27: Step 3. Tier discount = (list subtotal - volume discount) * tier rate, rounded to the cent.
    tier_discount = round_cent((list_subtotal - volume_discount) * TIER_RATES[customer_tier])

    # requirements.md line 28: Step 4. Discounted subtotal = list subtotal - volume discount - tier discount.
    discounted_subtotal = list_subtotal - volume_discount - tier_discount

    # requirements.md lines 29-33: Step 5. Coupon discount, based on the discounted subtotal.
    if coupon == "NONE":
        # requirements.md line 30: NONE: 0.
        coupon_discount = Decimal(0)
    elif coupon == "SAVE15":
        # requirements.md lines 31-32: SAVE15: >= 100.00 -> 15% rounded to cent, capped at 40.00; < 100.00 -> 0.
        if discounted_subtotal >= Decimal("100.00"):
            coupon_discount = min(round_cent(discounted_subtotal * Decimal("0.15")), Decimal(40))
        else:
            coupon_discount = Decimal(0)
    elif coupon == "FLAT25":
        # requirements.md line 33: FLAT25: 25.00 if discounted subtotal is 200.00 or more; otherwise 0.
        coupon_discount = Decimal(25) if discounted_subtotal >= Decimal("200.00") else Decimal(0)
    else:
        raise ValueError(f"Unknown coupon: {coupon!r}")

    # requirements.md line 34: Step 6. Merchandise total = discounted subtotal - coupon discount.
    merchandise_total = discounted_subtotal - coupon_discount

    # requirements.md lines 35-36: Step 7. Shipping: 0 for gold; for other tiers, 0 if merchandise total >= 75.00, else 6.95.
    if customer_tier == "gold" or merchandise_total >= Decimal("75.00"):
        shipping = Decimal(0)
    else:
        shipping = Decimal("6.95")

    # requirements.md line 37: Step 8. Total = merchandise total + shipping.
    total = merchandise_total + shipping

    # requirements.md lines 41-42: Output record with five fields.
    return {
        "volume_discount": volume_discount,
        "tier_discount": tier_discount,
        "coupon_discount": coupon_discount,
        "shipping": shipping,
        "total": total,
    }
