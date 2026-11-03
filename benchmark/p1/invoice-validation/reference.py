"""Hand-written reference implementation, written from requirements.md only.

Used by gen.py to place cases next to the derived thresholds, by the tests to check examples.json, and by oracle.py to
cross-check the oracle (written later, by another model family, from requirements.md alone). Not shown to either arm.
"""
from decimal import ROUND_HALF_EVEN, Decimal

D = Decimal
CENT = D("0.01")
TAX_RATE = {"DE": D("0.19"), "NL": D("0.21"), "UK": D("0.20"), "CH": D("0.081")}
TOLERANCE = D("0.02")
MAX_LINES = 200
MAX_AGE_DAYS = 180
MAX_TERMS = 60
INVOICE_LIMIT = D("200000.00")
ROLE_LIMIT = {"clerk": D("5000.00"), "manager": D("50000.00"), "director": None}
REASONS = ["LINE_COUNT", "FUTURE_DATED", "STALE", "TERMS", "OVERDUE", "TAX_MISMATCH", "OVER_LIMIT"]


def computed_tax(region, net_amount):
    return (net_amount * TAX_RATE[region]).quantize(CENT, ROUND_HALF_EVEN)


def failed(region, net_amount, declared_tax, line_count, invoice_day, received_day, payment_terms_days):
    """{reason code: whether that rule fails}."""
    tax = computed_tax(region, net_amount)
    return {
        "LINE_COUNT": not 1 <= line_count <= MAX_LINES,
        "FUTURE_DATED": invoice_day > received_day,
        "STALE": received_day - invoice_day > MAX_AGE_DAYS,
        "TERMS": payment_terms_days > MAX_TERMS,
        "OVERDUE": invoice_day + payment_terms_days < received_day,
        "TAX_MISMATCH": abs(declared_tax - tax) > TOLERANCE,
        "OVER_LIMIT": net_amount + tax > INVOICE_LIMIT,
    }


def validate_invoice(region, net_amount, declared_tax, line_count, invoice_day, received_day, payment_terms_days,
                     approver_role):
    tax = computed_tax(region, net_amount)
    gross = net_amount + tax
    bad = failed(region, net_amount, declared_tax, line_count, invoice_day, received_day, payment_terms_days)
    reasons = [code for code in REASONS if bad[code]]
    limit = ROLE_LIMIT[approver_role]
    return {
        "accepted": not reasons,
        "reasons": reasons,
        "computed_tax": tax,
        "gross_total": gross,
        "manual_approval": not reasons and limit is not None and gross > limit,
    }
