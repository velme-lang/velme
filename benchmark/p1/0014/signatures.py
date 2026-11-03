"""Function signatures to implement in solution.py (all in one module).

Records are plain dicts keyed by the field names below (snake_case). Number inputs arrive as decimal.Decimal, Text as
str. Return numbers as Decimal or int, exact, never float. A float anywhere in a result is a failure. Lists are Python
lists.

LoanProduct:    lender_name, rate, points, fee
Metric:         lender_name, rate, points, fee, loan_amt, down_pmt_amt, payment_amt, equity_36_mo_pct
RankedProducts: metrics_table, rank_by_down_pmt, rank_by_monthly_pmt, rank_by_equity_pct (each a list of Metric)
"""
from decimal import Decimal


def bankrates() -> list[dict]:
    """The lender table: ten LoanProduct dicts in table order."""
    raise NotImplementedError


def metrics_table(products: list[dict], requested_amt: Decimal | int) -> list[dict]:
    """One Metric dict per product, in the products' order."""
    raise NotImplementedError


def rank_metrics(table: list[dict]) -> dict:
    """A RankedProducts dict: the table itself and its three rankings."""
    raise NotImplementedError


def rank_products(requested_amt: Decimal | int) -> dict:
    """The RankedProducts dict for the whole lender table."""
    raise NotImplementedError
