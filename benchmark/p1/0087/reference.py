"""Hand-written reference implementation, written from requirements.md only (never from the DMN).

Used by gen_inputs.py (installment and risk categories), as the "correct reply" in the run_b2.py fake-reply
test, and by oracle.py to cross-check the oracle output against requirements.md. Not shown to either arm.
"""
from decimal import Decimal

D = Decimal


def application_risk_score(applicant):
    age = applicant["age"]
    pts = 0 if age < 18 else 32 if age < 22 else 35 if age < 26 else 40 if age < 36 else 43 if age < 50 else 48
    pts += {"S": 25, "M": 45}[applicant["marital_status"]]
    pts += {"UNEMPLOYED": 15, "STUDENT": 18, "EMPLOYED": 45, "SELF-EMPLOYED": 36}[applicant["employment_status"]]
    return D(pts)


def pre_bureau_risk_category(existing_customer, score):
    if existing_customer:
        return "DECLINE" if score < 80 else "HIGH" if score < 90 else "MEDIUM" if score <= 110 else "LOW"
    return "HIGH" if score < 100 else "MEDIUM" if score < 120 else "LOW" if score <= 130 else "VERY LOW"


def required_monthly_installment(product):
    fee = {"STANDARD LOAN": D("20.00"), "SPECIAL LOAN": D("25.00")}[product["product_type"]]
    r = product["rate"] / 12
    return (product["amount"] * r) / (1 - (1 + r) ** (-product["term"])) + fee


FACTOR = {"HIGH": D("0.6"), "DECLINE": D("0.6"), "MEDIUM": D("0.7"), "LOW": D("0.8"), "VERY LOW": D("0.8")}


def affordability(applicant, risk_category, installment):
    disposable = applicant["monthly_income"] - (applicant["monthly_repayments"] + applicant["monthly_expenses"])
    return disposable * FACTOR[risk_category] > installment


def eligibility(risk_category, affordable, age):
    return "INELIGIBLE" if risk_category == "DECLINE" or not affordable or age < 18 else "ELIGIBLE"


def bureau_call_type(risk_category):
    return {"HIGH": "FULL", "MEDIUM": "FULL", "LOW": "MINI", "VERY LOW": "NONE", "DECLINE": "NONE"}[risk_category]


def strategy(eligibility, bureau_call_type):
    if eligibility == "INELIGIBLE":
        return "DECLINE"
    return "THROUGH" if bureau_call_type == "NONE" else "BUREAU"


def post_bureau_risk_category(existing_customer, score, credit_score):
    if existing_customer:
        lo, hi = (580, 600) if score <= 100 else (590, 615)
    elif score > 130:
        return "VERY LOW"
    else:
        lo, hi = (590, 610) if score < 120 else (600, 625)
    return "HIGH" if credit_score < lo else "MEDIUM" if credit_score <= hi else "LOW"


def routing(risk_category, affordable, bankrupt, credit_score):
    if not affordable or bankrupt:
        return "DECLINE"
    return "REFER" if risk_category == "HIGH" or credit_score < 580 else "ACCEPT"


def decide_loan(applicant, product, bureau):
    score = application_risk_score(applicant)
    pre = pre_bureau_risk_category(applicant["existing_customer"], score)
    inst = required_monthly_installment(product)
    pre_aff = affordability(applicant, pre, inst)
    elig = eligibility(pre, pre_aff, applicant["age"])
    call = bureau_call_type(pre)
    post = post_bureau_risk_category(applicant["existing_customer"], score, bureau["credit_score"])
    post_aff = affordability(applicant, post, inst)
    return {
        "application_risk_score": score,
        "pre_bureau_risk_category": pre,
        "required_monthly_installment": inst,
        "pre_bureau_affordability": pre_aff,
        "eligibility": elig,
        "bureau_call_type": call,
        "strategy": strategy(elig, call),
        "post_bureau_risk_category": post,
        "post_bureau_affordability": post_aff,
        "routing": routing(post, post_aff, bureau["bankrupt"], bureau["credit_score"]),
    }
