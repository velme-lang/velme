"""Independent Python oracle for insurance-quote, derived solely from requirements.md."""
from decimal import Decimal, ROUND_HALF_UP

# requirements.md lines 20-21: "Rounded to the cent" = two decimal places, half up.
CENT = Decimal("0.01")


def round_cent(amount: Decimal) -> Decimal:
    return amount.quantize(CENT, rounding=ROUND_HALF_UP)


# requirements.md lines 23-30: Step 1. Base rate by vehicle group and age band (17-24, 25-64, 65-90 inclusive).
BASE_RATES = {
    "A": (Decimal(620), Decimal(380), Decimal(420)),
    "B": (Decimal(780), Decimal(450), Decimal(510)),
    "C": (Decimal(1050), Decimal(590), Decimal(680)),
    "D": (Decimal(1600), Decimal(820), Decimal(990)),
}


def _age_band_index(driver_age: Decimal) -> int:
    # requirements.md line 23: Ages 17 to 24, 25 to 64, 65 to 90, each band including both ends.
    if driver_age <= Decimal(24):
        return 0
    if driver_age <= Decimal(64):
        return 1
    return 2


def _claims_factor(claims: Decimal) -> Decimal:
    # requirements.md line 33: Claims factor: 0 -> 1.00; 1 -> 1.25; 2 -> 1.60; 3, 4 or 5 -> 2.20.
    if claims == Decimal(0):
        return Decimal("1.00")
    if claims == Decimal(1):
        return Decimal("1.25")
    if claims == Decimal(2):
        return Decimal("1.60")
    return Decimal("2.20")


def _mileage_loading(annual_mileage: Decimal) -> Decimal:
    # requirements.md line 34: Mileage loading: 0..20000 -> 0; 20001..35000 -> 60.00; 35001..60000 -> 150.00.
    if annual_mileage <= Decimal(20000):
        return Decimal("0")
    if annual_mileage <= Decimal(35000):
        return Decimal("60.00")
    return Decimal("150.00")


def _young_driver_factor(driver_age: Decimal) -> Decimal:
    # requirements.md line 35: Young-driver factor: 1.15 for ages 17 to 20 inclusive; 1.00 otherwise.
    if Decimal(17) <= driver_age <= Decimal(20):
        return Decimal("1.15")
    return Decimal("1.00")


def decide(case: dict) -> dict:
    driver_age = Decimal(case["driver_age"])
    vehicle_group = case["vehicle_group"]
    claims = Decimal(case["claims_last_5_years"])
    annual_mileage = Decimal(case["annual_mileage"])
    has_telematics = bool(case["has_telematics"])

    # requirements.md lines 23-30: Step 1. Base rate by vehicle group and age band.
    base_rate = BASE_RATES[vehicle_group][_age_band_index(driver_age)]

    # requirements.md lines 32-35: Step 2. Loaded premium = (base rate * claims factor + mileage loading) * young-driver factor, rounded to the cent.
    loaded_premium = round_cent(
        (base_rate * _claims_factor(claims) + _mileage_loading(annual_mileage))
        * _young_driver_factor(driver_age)
    )

    # requirements.md lines 36-38: Step 3. Discount amount = loaded premium * (1 - discount multiplier), rounded to the cent.
    telematics_mult = Decimal("0.90") if has_telematics else Decimal(1)
    claims_mult = Decimal("0.85") if claims == Decimal(0) else Decimal(1)
    discount_multiplier = telematics_mult * claims_mult
    discount_amount = round_cent(loaded_premium * (Decimal(1) - discount_multiplier))

    # requirements.md lines 39-41: Step 4. Annual premium = loaded premium - discount amount, clamped to [300.00, 3500.00].
    raw_annual = loaded_premium - discount_amount
    if raw_annual < Decimal("300.00"):
        annual_premium = Decimal(300)
    elif raw_annual > Decimal("3500.00"):
        annual_premium = Decimal(3500)
    else:
        annual_premium = raw_annual

    # requirements.md line 42: Step 5. Monthly instalment = annual premium * 1.05 / 12, rounded to the cent.
    monthly_instalment = round_cent(annual_premium * Decimal("1.05") / Decimal(12))

    # requirements.md lines 46-47: Output record with five fields.
    return {
        "base_rate": base_rate,
        "loaded_premium": loaded_premium,
        "discount_amount": discount_amount,
        "annual_premium": annual_premium,
        "monthly_instalment": monthly_instalment,
    }
