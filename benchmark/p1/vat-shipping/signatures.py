"""Function signatures to implement in solution.py (all in one module).

Number inputs arrive as decimal.Decimal; text inputs as str. Return numbers as Decimal or int, exact, never float. A
float anywhere in a result is a failure.

Totals: shipping, goods_vat, shipping_vat, total_gross
"""
from decimal import Decimal


def order_totals(region: str, category: str, net_amount: Decimal, weight_kg: Decimal) -> dict:
    """Return a totals dict with all four fields."""
    raise NotImplementedError
