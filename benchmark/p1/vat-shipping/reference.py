"""Hand-written reference implementation, written from requirements.md only.

Used by gen.py's tests, by the tests to check examples.json, and by oracle.py to cross-check the oracle (written
later, by another model family, from requirements.md alone). Not shown to either arm.
"""
from decimal import ROUND_HALF_EVEN, Decimal

D = Decimal
CENT = D("0.01")
VAT = {
    "home": {"standard": D("0.19"), "books": D("0.07"), "food": D("0.07")},
    "eu": {"standard": D("0.21"), "books": D("0.06"), "food": D("0.09")},
    "uk": {"standard": D("0.20"), "books": D(0), "food": D("0.05")},
    "world": {"standard": D(0), "books": D(0), "food": D(0)},
}
SHIPPING = {  # by weight band: up to 0.500 kg, up to 2.000, up to 10.000, up to 30.000 (each inclusive)
    "home": [D("4.50"), D("6.90"), D("11.50"), D("24.00")],
    "eu": [D("9.50"), D("15.00"), D("26.00"), D("48.00")],
    "uk": [D("12.00"), D("19.50"), D("34.00"), D("65.00")],
    "world": [D("18.00"), D("29.00"), D("55.00"), D("110.00")],
}
BANDS = [D("0.5"), D(2), D(10), D(30)]
FREE_FROM = {"home": D(50), "eu": D(150)}


def cents(x):
    return x.quantize(CENT, ROUND_HALF_EVEN)


def shipping_charge(region, net_amount, weight_kg):
    if region in FREE_FROM and net_amount >= FREE_FROM[region]:
        return D(0)
    return SHIPPING[region][next(i for i, top in enumerate(BANDS) if weight_kg <= top)]


def order_totals(region, category, net_amount, weight_kg):
    shipping = shipping_charge(region, net_amount, weight_kg)
    rate = VAT[region][category]
    shipping_rate = VAT["uk"]["standard"] if region == "uk" else rate
    goods_vat = cents(net_amount * rate)
    shipping_vat = cents(shipping * shipping_rate)
    return {
        "shipping": shipping,
        "goods_vat": goods_vat,
        "shipping_vat": shipping_vat,
        "total_gross": net_amount + shipping + goods_vat + shipping_vat,
    }
