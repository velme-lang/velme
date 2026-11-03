"""Function signatures to implement in solution.py (all in one module).

Number inputs arrive as decimal.Decimal. Return numbers as Decimal or int, exact, never float. A float anywhere in a
result is a failure.

Vacation: base_vacation_days, extra_days_case_1, extra_days_case_2, extra_days_case_3, total_vacation_days
"""
from decimal import Decimal


def extra_days_case_1(age: Decimal | int, years_of_service: Decimal | int) -> Decimal | int:
    raise NotImplementedError


def extra_days_case_2(age: Decimal | int, years_of_service: Decimal | int) -> Decimal | int:
    raise NotImplementedError


def extra_days_case_3(age: Decimal | int, years_of_service: Decimal | int) -> Decimal | int:
    raise NotImplementedError


def vacation_days(age: Decimal | int, years_of_service: Decimal | int) -> dict:
    """Return a Vacation dict with all five fields."""
    raise NotImplementedError
