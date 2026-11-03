"""0014's Drools adapter: held-out case -> DMN context, the DMN result -> the descriptor's four lists, and the
reference cross-check oracle.py runs."""
from decimal import Decimal

import reference

context_defaults = {}
Q = Decimal("1e-8")
LISTS = {"metricsTable": "metrics_table", "rankByDownPmt": "rank_by_down_pmt", "rankByMonthlyPmt": "rank_by_monthly_pmt",
         "rankByEquityPct": "rank_by_equity_pct"}
KEYS = {"lenderName": "lender_name", "rate": "rate", "points": "points", "fee": "fee", "loanAmt": "loan_amt",
        "downPmtAmt": "down_pmt_amt", "paymentAmt": "payment_amt", "equity36moPct": "equity_36_mo_pct"}


def to_context(case):
    return {"RequestedAmt": case["requested_amt"]}


def from_dmn(values):
    ranked = values["ranked_products"]
    return {LISTS[k]: [{KEYS[c]: m[c] for c in KEYS} for m in ranked[k]] for k in LISTS}


def rounded(v):
    """Every number to 8 places, through lists and records; the TCK lists double-precision values (about 16 digits)."""
    if isinstance(v, list):
        return [rounded(x) for x in v]
    if isinstance(v, dict):
        return {k: rounded(x) for k, x in v.items()}
    return v.quantize(Q) if isinstance(v, Decimal) else v


def tck_equal(name, got, want):
    return rounded(got) == rounded(want)


def check(case, row):
    """A string when requirements.md and the DMN disagree beyond the 8-place payment and equity comparison, else None."""
    want = rounded(reference.rank_products(case["requested_amt"]))
    got = rounded({k: row[k] for k in want})
    return None if got == want else f"reference and DMN differ:\n  dmn {row}\n  ref {want}"
