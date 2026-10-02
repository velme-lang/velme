"""Shared helpers: held-out inputs, exact-decimal JSON, trial output files."""
import json
from decimal import Decimal
from pathlib import Path

HERE = Path(__file__).parent
CASE = HERE / "0087"
HELDOUT = CASE / "heldout"
FIELDS = ["application_risk_score", "pre_bureau_risk_category", "required_monthly_installment", "pre_bureau_affordability",
          "eligibility", "bureau_call_type", "strategy", "post_bureau_risk_category", "post_bureau_affordability", "routing"]


def loads(text):
    """json.loads with every number an exact Decimal."""
    return json.loads(text, parse_float=Decimal, parse_int=Decimal)


def dumps(v):
    """JSON with Decimal written as its exact plain text (float is written as is, so the scorer can see it)."""
    if isinstance(v, Decimal):
        return format(v, "f")
    if isinstance(v, dict):
        return "{" + ", ".join(f"{json.dumps(k)}: {dumps(x)}" for k, x in v.items()) + "}"
    if isinstance(v, list):
        return "[" + ", ".join(dumps(x) for x in v) + "]"
    return json.dumps(v)


def load_inputs():
    return [loads(l) for l in (HELDOUT / "inputs.jsonl").read_text().splitlines() if l.strip()]


def write_trial(out_dir, trial, rows, meta):
    """rows: [{"id", "output": dict | None, "error": str | None}] in input order."""
    d = Path(out_dir) / f"trial-{trial}"
    d.mkdir(parents=True, exist_ok=True)
    (d / "results.jsonl").write_text("".join(dumps(r) + "\n" for r in rows))
    (d / "meta.json").write_text(json.dumps(meta, indent=1) + "\n")
