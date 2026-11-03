"""Function signatures to implement in solution.py (all in one module).

Number inputs arrive as decimal.Decimal; text inputs as str. Return numbers as Decimal or int, exact, never float;
accepted and manual_approval as bool; reasons as a list of str. A float anywhere in a result is a failure.

Invoice decision: accepted, reasons, computed_tax, gross_total, manual_approval
"""
from decimal import Decimal


def validate_invoice(region: str, net_amount: Decimal, declared_tax: Decimal, line_count: Decimal | int,
                     invoice_day: Decimal | int, received_day: Decimal | int, payment_terms_days: Decimal | int,
                     approver_role: str) -> dict:
    """Return an invoice decision dict with all five fields."""
    raise NotImplementedError
