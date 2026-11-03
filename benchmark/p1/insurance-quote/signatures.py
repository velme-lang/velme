"""Function signatures to implement in solution.py (all in one module).

Number inputs arrive as decimal.Decimal; text inputs as str; has_telematics as bool. Return numbers as Decimal or
int, exact, never float. A float anywhere in a result is a failure.

Quote: base_rate, loaded_premium, discount_amount, annual_premium, monthly_instalment
"""
from decimal import Decimal


def quote(driver_age: Decimal | int, vehicle_group: str, claims_last_5_years: Decimal | int,
          annual_mileage: Decimal | int, has_telematics: bool) -> dict:
    """Return a quote dict with all five fields."""
    raise NotImplementedError
