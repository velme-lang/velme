"""Hand-written reference implementation, written from requirements.md only.

Used by gen.py to place cases next to the minimum and the cap, by the tests to check examples.json, and by oracle.py
to cross-check the oracle (written later, by another model family, from requirements.md alone). Not shown to either
arm.
"""
from decimal import ROUND_HALF_UP, Decimal

D = Decimal
CENT = D("0.01")
BASE = {  # by age band: 17-24, 25-64, 65-90
    "A": [D(620), D(380), D(420)],
    "B": [D(780), D(450), D(510)],
    "C": [D(1050), D(590), D(680)],
    "D": [D(1600), D(820), D(990)],
}
MINIMUM, CAP = D(300), D(3500)


def cents(x):
    return x.quantize(CENT, ROUND_HALF_UP)


def base_rate(driver_age, vehicle_group):
    return BASE[vehicle_group][0 if driver_age <= 24 else 1 if driver_age <= 64 else 2]


def claims_factor(claims):
    return [D(1), D("1.25"), D("1.60")][int(claims)] if claims < 3 else D("2.20")


def mileage_loading(annual_mileage):
    return D(0) if annual_mileage <= 20000 else D(60) if annual_mileage <= 35000 else D(150)


def before_limits(driver_age, vehicle_group, claims_last_5_years, annual_mileage, has_telematics):
    """(base rate, loaded premium, discount amount, premium before the minimum and cap)."""
    base = base_rate(driver_age, vehicle_group)
    loaded = base * claims_factor(claims_last_5_years) + mileage_loading(annual_mileage)
    if driver_age <= 20:
        loaded *= D("1.15")
    loaded = cents(loaded)
    keep = (D("0.90") if has_telematics else D(1)) * (D("0.85") if claims_last_5_years == 0 else D(1))
    discount = cents(loaded * (1 - keep))
    return base, loaded, discount, loaded - discount


def quote(driver_age, vehicle_group, claims_last_5_years, annual_mileage, has_telematics):
    base, loaded, discount, net = before_limits(driver_age, vehicle_group, claims_last_5_years, annual_mileage,
                                                has_telematics)
    annual = min(max(net, MINIMUM), CAP)
    return {
        "base_rate": base,
        "loaded_premium": loaded,
        "discount_amount": discount,
        "annual_premium": annual,
        "monthly_instalment": cents(annual * D("1.05") / 12),
    }
