"""Independent refund decisions from requirements.md."""

from decimal import Decimal, ROUND_HALF_UP


def decide(case):
    category = case["category"]
    membership = case["membership"]
    days = case["days_since_purchase"]
    condition = case["condition"]
    price = case["price"]

    # requirements.md:24-27: category window plus membership extension, inclusive.
    category_days = {"electronics": 30, "apparel": 60, "furniture": 14}
    extra_days = {"none": 0, "plus": 15, "premier": 30}
    window_days = category_days[category] + extra_days[membership]

    # requirements.md:46-49: no refund or fee unless the matching case grants it.
    refund_type = "none"
    amount = Decimal("0.00")
    restocking_fee = Decimal("0.00")

    # requirements.md:29-33: warranty takes precedence, including day 180.
    if case["defective"] and days <= 180:
        refund_type = "full"
        amount = price
    elif days <= window_days:
        # requirements.md:35: unopened returns within the window get full price.
        if condition == "unopened":
            refund_type = "full"
            amount = price
        elif condition == "opened":
            # requirements.md:36-38: adjust the rate, round half-up, then cap.
            rates = {
                "electronics": Decimal("0.15"),
                "furniture": Decimal("0.20"),
                "apparel": Decimal("0"),
            }
            factors = {
                "none": Decimal("1"),
                "plus": Decimal("0.5"),
                "premier": Decimal("0"),
            }
            rounded_fee = (price * rates[category] * factors[membership]).quantize(
                Decimal("0.01"), rounding=ROUND_HALF_UP
            )
            restocking_fee = min(rounded_fee, Decimal("75.00"))
            amount = price - restocking_fee
            refund_type = "full" if restocking_fee == 0 else "partial"
        elif category != "apparel":
            # requirements.md:21-22,39: used apparel gets none; others get 50% credit.
            refund_type = "store_credit"
            amount = (price * Decimal("0.50")).quantize(
                Decimal("0.01"), rounding=ROUND_HALF_UP
            )
    elif days <= window_days + 30 and condition != "used":
        # requirements.md:21-22,26-27,40-41: grace gives 80% credit without a fee.
        refund_type = "store_credit"
        amount = (price * Decimal("0.80")).quantize(
            Decimal("0.01"), rounding=ROUND_HALF_UP
        )
    # requirements.md:39,41-42: used apparel, used grace, and later returns get none.

    # requirements.md:46-49: report the window even when warranty or no refund applies.
    return {
        "refund_type": refund_type,
        "amount": amount,
        "restocking_fee": restocking_fee,
        "window_days": window_days,
    }
