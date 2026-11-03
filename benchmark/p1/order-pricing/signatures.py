"""Function signatures to implement in solution.py (all in one module).

Number inputs arrive as decimal.Decimal; text inputs as str. Return numbers as Decimal or int, exact, never float. A
float anywhere in a result is a failure.

Order price: volume_discount, tier_discount, coupon_discount, shipping, total
"""
from decimal import Decimal


def price_order(customer_tier: str, unit_price: Decimal, quantity: Decimal | int, coupon: str) -> dict:
    """Return an order price dict with all five fields."""
    raise NotImplementedError
