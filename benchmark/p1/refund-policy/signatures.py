"""Function signatures to implement in solution.py (all in one module).

Number inputs arrive as decimal.Decimal; text inputs as str; defective as bool. Return numbers as Decimal or int,
exact, never float. A float anywhere in a result is a failure.

Refund: refund_type, amount, restocking_fee, window_days
"""
from decimal import Decimal


def refund(category: str, membership: str, days_since_purchase: Decimal | int, condition: str, defective: bool,
           price: Decimal) -> dict:
    """Return a refund dict with all four fields."""
    raise NotImplementedError
