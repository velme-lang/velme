"""Function signatures to implement in solution.py (all in one module).

Records are plain dicts keyed by the field names below, exactly as in requirements.md (snake_case).
Number inputs arrive as decimal.Decimal, Text as str, Boolean as bool. Return numbers as Decimal or int,
exact, never float. A float anywhere in a result is a failure. Nothing is rounded.

Applicant: age, marital_status, employment_status, existing_customer, monthly_income, monthly_repayments,
           monthly_expenses
Product:   product_type, rate, term, amount
Bureau:    bankrupt, credit_score
Decision:  application_risk_score, pre_bureau_risk_category, required_monthly_installment,
           pre_bureau_affordability, eligibility, bureau_call_type, strategy, post_bureau_risk_category,
           post_bureau_affordability, routing
"""
from decimal import Decimal


def application_risk_score(applicant: dict) -> Decimal | int:
    raise NotImplementedError


def pre_bureau_risk_category(existing_customer: bool, score: Decimal | int) -> str:
    raise NotImplementedError


def required_monthly_installment(product: dict) -> Decimal | int:
    raise NotImplementedError


def affordability(applicant: dict, risk_category: str, installment: Decimal | int) -> bool:
    raise NotImplementedError


def eligibility(risk_category: str, affordable: bool, age: Decimal | int) -> str:
    raise NotImplementedError


def bureau_call_type(risk_category: str) -> str:
    raise NotImplementedError


def strategy(eligibility: str, bureau_call_type: str) -> str:
    raise NotImplementedError


def post_bureau_risk_category(existing_customer: bool, score: Decimal | int, credit_score: Decimal | int) -> str:
    raise NotImplementedError


def routing(risk_category: str, affordable: bool, bankrupt: bool, credit_score: Decimal | int) -> str:
    raise NotImplementedError


def decide_loan(applicant: dict, product: dict, bureau: dict) -> dict:
    """Return a Decision dict with all ten fields."""
    raise NotImplementedError
