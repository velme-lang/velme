"""Hand-written reference implementation, written from requirements.md only (never from the DMN).

Used by gen.py (the ranking crossings), by the offline tests, and by oracle.py to cross-check the oracle output
against requirements.md. Not shown to either arm. Python's default 28-digit decimal context, like Velme's numbers.
"""
from decimal import Decimal

D = Decimal

# (lender_name, rate, points, fee), in table order.
TABLE = [
    ("Oceans Capital", "0.03500", "0", "0"),
    ("eClick Lending", "0.03200", "1.1", "2700"),
    ("eClickLending", "0.03375", "0.1", "1200"),
    ("AimLoan", "0.03000", "1.1", "3966"),
    ("Home Loans Today", "0.03125", "1.1", "285"),
    ("Sebonic", "0.03125", "0.1", "4028"),
    ("AimLoan", "0.03125", "0.1", "4317"),
    ("eRates Mortgage", "0.03125", "1.1", "2518"),
    ("Home Loans Today", "0.03250", "0.1", "822"),
    ("AimLoan", "0.03250", "0", "1995"),
]


def bankrates():
    return [{"lender_name": n, "rate": D(r), "points": D(p), "fee": D(f)} for n, r, p, f in TABLE]


def monthly_payment(p, r, n):
    return p * r / 12 / (1 - (1 + r / 12) ** -n)


def equity_36_mo(p, r, n, pmt):
    return p * (1 + r / 12) ** n - pmt * (-1 + (1 + r / 12) ** n) / r


def financial_metrics(product, requested_amt):
    rate = product["rate"]
    loan = requested_amt * (1 + product["points"] / 100) + product["fee"]
    payment = monthly_payment(loan, rate, 360)
    return {
        **product,
        "loan_amt": loan,
        "down_pmt_amt": D("0.2") * loan,
        "payment_amt": payment,
        "equity_36_mo_pct": 1 - equity_36_mo(loan, rate, 36, payment) / requested_amt * D("0.8"),
    }


def metrics_table(products, requested_amt):
    return [financial_metrics(p, requested_amt) for p in products]


def rank_metrics(table):
    return {
        "metrics_table": table,
        "rank_by_down_pmt": sorted(table, key=lambda m: m["down_pmt_amt"]),  # sorted() is stable: ties keep table order
        "rank_by_monthly_pmt": sorted(table, key=lambda m: m["payment_amt"]),
        "rank_by_equity_pct": sorted(table, key=lambda m: m["equity_36_mo_pct"], reverse=True),  # reverse stays stable
    }


def rank_products(requested_amt):
    return rank_metrics(metrics_table(bankrates(), requested_amt))
