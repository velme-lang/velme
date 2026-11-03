"""Independent invoice decisions from requirements.md."""

from decimal import Decimal, ROUND_HALF_EVEN


def decide(case):
    # requirements.md:25-29: exact regional tax, rounded once to cents half-even.
    rates = {
        "DE": Decimal("0.19"),
        "NL": Decimal("0.21"),
        "UK": Decimal("0.20"),
        "CH": Decimal("0.081"),
    }
    computed_tax = (case["net_amount"] * rates[case["region"]]).quantize(
        Decimal("0.01"), rounding=ROUND_HALF_EVEN
    )
    # requirements.md:30-31: gross uses computed tax; due day uses invoice day.
    gross_total = case["net_amount"] + computed_tax
    due_day = case["invoice_day"] + case["payment_terms_days"]

    # requirements.md:35-37: evaluate every rule, preserving table order.
    failures = [
        # requirements.md:41: both line-count endpoints are inclusive.
        ("LINE_COUNT", case["line_count"] == 0 or case["line_count"] > 200),
        # requirements.md:42: an invoice issued on receipt day is valid.
        ("FUTURE_DATED", case["invoice_day"] > case["received_day"]),
        # requirements.md:43: exactly 180 days old is valid.
        ("STALE", case["received_day"] - case["invoice_day"] > 180),
        # requirements.md:44: exactly 60 days of payment terms is valid.
        ("TERMS", case["payment_terms_days"] > 60),
        # requirements.md:45: payment due on receipt day is valid.
        ("OVERDUE", due_day < case["received_day"]),
        # requirements.md:46: allow a two-cent difference in either direction.
        ("TAX_MISMATCH", abs(case["declared_tax"] - computed_tax) > Decimal("0.02")),
        # requirements.md:47: the gross limit itself is valid.
        ("OVER_LIMIT", gross_total > Decimal("200000.00")),
    ]
    reasons = [code for code, failed in failures if failed]
    # requirements.md:49: acceptance requires every rejection rule to pass.
    accepted = not reasons

    # requirements.md:51-53: only accepted invoices above a non-director limit.
    limits = {"clerk": Decimal("5000.00"), "manager": Decimal("50000.00")}
    role = case["approver_role"]
    manual_approval = accepted and role != "director" and gross_total > limits[role]

    # requirements.md:57-59: return exactly these five fields, even on rejection.
    return {
        "accepted": accepted,
        "reasons": reasons,
        "computed_tax": computed_tax,
        "gross_total": gross_total,
        "manual_approval": manual_approval,
    }
