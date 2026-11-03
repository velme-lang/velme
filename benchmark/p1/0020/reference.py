"""Hand-written reference implementation, written from requirements.md only (never from the DMN).

Used by gen.py's tests, as the "correct reply" in a fake-reply run, and by oracle.py to cross-check the oracle output
against requirements.md. Not shown to either arm.
"""
from decimal import Decimal

D = Decimal
BASE = D(22)


def extra_days_case_1(age, years_of_service):
    return D(5) if age < 18 or age >= 60 or years_of_service >= 30 else D(0)


def extra_days_case_2(age, years_of_service):
    return D(3) if years_of_service >= 30 or age >= 60 else D(0)


def extra_days_case_3(age, years_of_service):
    return D(2) if 15 <= years_of_service < 30 or age >= 45 else D(0)


def vacation_days(age, years_of_service):
    one, two, three = (f(age, years_of_service) for f in (extra_days_case_1, extra_days_case_2, extra_days_case_3))
    return {
        "base_vacation_days": BASE,
        "extra_days_case_1": one,
        "extra_days_case_2": two,
        "extra_days_case_3": three,
        "total_vacation_days": BASE + max(one, three) + two,
    }
