"""Independent Python oracle for vat-shipping, derived solely from requirements.md."""
from decimal import Decimal, ROUND_HALF_EVEN

# requirements.md lines 44-46: "Rounded to the cent" = two decimal places, half to even.
CENT = Decimal("0.01")


def round_cent(amount: Decimal) -> Decimal:
    return amount.quantize(CENT, rounding=ROUND_HALF_EVEN)


# requirements.md lines 20-25: VAT rates by region and category.
GOODS_VAT_RATES = {
    "home": {"standard": Decimal("0.19"), "books": Decimal("0.07"), "food": Decimal("0.07")},
    "eu": {"standard": Decimal("0.21"), "books": Decimal("0.06"), "food": Decimal("0.09")},
    "uk": {"standard": Decimal("0.20"), "books": Decimal("0"), "food": Decimal("0.05")},
    "world": {"standard": Decimal("0"), "books": Decimal("0"), "food": Decimal("0")},
}

# requirements.md lines 32-37: Shipping net rates by region and weight band.
SHIPPING_TABLE = {
    "home": (Decimal("4.50"), Decimal("6.90"), Decimal("11.50"), Decimal("24.00")),
    "eu": (Decimal("9.50"), Decimal("15.00"), Decimal("26.00"), Decimal("48.00")),
    "uk": (Decimal("12.00"), Decimal("19.50"), Decimal("34.00"), Decimal("65.00")),
    "world": (Decimal("18.00"), Decimal("29.00"), Decimal("55.00"), Decimal("110.00")),
}


def _weight_band_index(weight_kg: Decimal) -> int:
    # requirements.md lines 29-30: First band the weight fits: <= 0.500, <= 2.000, <= 10.000, <= 30.000 kg.
    if weight_kg <= Decimal("0.500"):
        return 0
    if weight_kg <= Decimal("2.000"):
        return 1
    if weight_kg <= Decimal("10.000"):
        return 2
    return 3


def _shipping_net(region: str, net_amount: Decimal, weight_kg: Decimal) -> Decimal:
    # requirements.md lines 39-40: Free (0) for home when net_amount >= 50.00, and for eu when net_amount >= 150.00;
    # never free for uk or world.
    if region == "home" and net_amount >= Decimal("50.00"):
        return Decimal(0)
    if region == "eu" and net_amount >= Decimal("150.00"):
        return Decimal(0)
    return SHIPPING_TABLE[region][_weight_band_index(weight_kg)]


def _shipping_vat_rate(region: str, goods_vat_rate: Decimal) -> Decimal:
    # requirements.md lines 49-50: Shipping VAT rate: home and eu same as goods; uk always 20%; world 0%.
    if region in ("home", "eu"):
        return goods_vat_rate
    if region == "uk":
        return Decimal("0.20")
    if region == "world":
        return Decimal("0")
    raise ValueError(f"Unknown region: {region!r}")


def decide(case: dict) -> dict:
    region = case["region"]
    category = case["category"]
    net_amount = Decimal(case["net_amount"])
    weight_kg = Decimal(case["weight_kg"])

    goods_rate = GOODS_VAT_RATES[region][category]
    shipping = _shipping_net(region, net_amount, weight_kg)

    # requirements.md line 48: Step 1. Goods VAT = net amount * VAT rate for region and category, rounded to the cent.
    goods_vat = round_cent(net_amount * goods_rate)

    # requirements.md lines 49-50: Step 2. Shipping VAT = shipping * shipping VAT rate, rounded to the cent.
    shipping_vat = round_cent(shipping * _shipping_vat_rate(region, goods_rate))

    # requirements.md line 51: Step 3. Total gross = net amount + shipping + goods VAT + shipping VAT.
    total_gross = net_amount + shipping + goods_vat + shipping_vat

    # requirements.md lines 55-56: Output record with four fields.
    return {
        "shipping": shipping,
        "goods_vat": goods_vat,
        "shipping_vat": shipping_vat,
        "total_gross": total_gross,
    }
