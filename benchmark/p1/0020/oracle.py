"""0020's Drools adapter: held-out case -> DMN context, and the reference cross-check oracle.py runs."""
import reference

context_defaults = {}


def to_context(case):
    return {"Age": case["age"], "Years of Service": case["years_of_service"]}


def check(case, row):
    """A string when requirements.md and the DMN disagree, else None."""
    want = reference.vacation_days(case["age"], case["years_of_service"])
    got = {k: row[k] for k in want}
    return None if got == want else f"reference and DMN differ:\n  dmn {row}\n  ref {want}"
