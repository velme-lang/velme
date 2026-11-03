"""0087's Drools adapter: held-out case -> DMN context, and the reference cross-check oracle.py runs."""
from decimal import Decimal

import reference

# The one input the model declares and no TCK case supplies; only the Adjudication decision (which has no logic) uses it.
context_defaults = {"Supporting documents": ""}


def to_context(case):
    a, p, b = case["applicant"], case["product"], case["bureau"]
    return {
        "Applicant data": {"Age": a["age"], "MartitalStatus": a["marital_status"], "EmploymentStatus": a["employment_status"],
                           "ExistingCustomer": a["existing_customer"],
                           "Monthly": {"Income": a["monthly_income"], "Repayments": a["monthly_repayments"],
                                       "Expenses": a["monthly_expenses"]}},
        "Requested product": {"ProductType": p["product_type"], "Rate": p["rate"], "Term": p["term"], "Amount": p["amount"]},
        "Bureau data": {"Bankrupt": b["bankrupt"], "CreditScore": b["credit_score"]},
    }


def check(case, row):
    """A string when requirements.md and the DMN disagree beyond the 8-decimal installment comparison, else None."""
    q = lambda d: d["required_monthly_installment"].quantize(Decimal("1e-8"))  # noqa: E731
    want = reference.decide_loan(case["applicant"], case["product"], case["bureau"])
    got = {**{k: row[k] for k in want}, "required_monthly_installment": q(row)}
    expected = {**want, "required_monthly_installment": q(want)}
    return None if got == expected else f"reference and DMN differ:\n  dmn {row}\n  ref {want}"
