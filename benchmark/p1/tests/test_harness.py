"""Offline tests for the harness: score.py arithmetic, run_b2.py fake-reply loop, run_velme.py output parsing."""
import json
import sys
from argparse import Namespace
from decimal import Decimal as D
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))

import run_b2  # noqa: E402
import run_velme  # noqa: E402
import score  # noqa: E402
from common import CASE, FIELDS, dumps, loads  # noqa: E402

WANT = loads(json.dumps({
    "application_risk_score": 138, "pre_bureau_risk_category": "VERY LOW", "required_monthly_installment": "x",
    "pre_bureau_affordability": True, "eligibility": "ELIGIBLE", "bureau_call_type": "NONE", "strategy": "THROUGH",
    "post_bureau_risk_category": "VERY LOW", "post_bureau_affordability": True, "routing": "ACCEPT"}).replace('"x"', "20.123456789012345678"))


def case(**over):
    return {"output": {**WANT, **over}}


def test_pass():
    assert score.score_case(case(), WANT) == ([], False)


def test_wrong_field():
    assert score.score_case(case(routing="REFER"), WANT)[0] == ["routing"]
    assert score.score_case(case(pre_bureau_affordability=1), WANT)[0] == ["pre_bureau_affordability"]  # 1 is not a Boolean
    assert score.score_case(case(application_risk_score="138"), WANT)[0] == ["application_risk_score"]


def test_float_counts():
    row = {**case(), "float": True}
    assert score.score_case(row, WANT) == ([], True)


def test_installment_precision():
    # differs at the 10th decimal place -> equal after rounding to 8; at the 6th -> not equal
    assert score.score_case(case(required_monthly_installment=D("20.1234567891")), WANT)[0] == []
    assert score.score_case(case(required_monthly_installment=D("20.123457")), WANT)[0] == ["required_monthly_installment"]
    assert score.score_case({"output": None}, WANT)[0] == FIELDS


def test_trial_scoring(tmp_path):
    expected = {1: WANT, 2: WANT}
    (tmp_path / "results.jsonl").write_text(dumps({"id": 1, **case(), "error": None}) + "\n" + dumps({"id": 2, "output": None, "error": "x"}) + "\n")
    r = score.score_trial(tmp_path, expected, {1})
    assert (r["passed"], r["b_pass"], r["r_pass"], r["no_output"]) == (1, 1, 0, 1)


def test_cost():
    m = {"tokens_in": 1_000_000, "tokens_out": 1_000_000, "cache_read": 1_000_000, "cache_write": 1_000_000}
    assert abs(score.cost(m, 3, 15) - (3 + 15 + 0.3 + 3.75)) < 1e-9


def b2_args(name):
    return Namespace(model="fake", fake_reply=str(Path(__file__).parent / name))


def run_loop(name):
    system = run_b2.SYSTEM.format(requirements="r", signatures="s", examples="e")
    return run_b2.model_loop(b2_args(name), system)


def test_b2_first_reply_passes():
    _, _, passed, use, _ = run_loop("fake_ok")
    assert passed and use["calls"] == 1


def test_b2_fix_round():
    _, _, passed, use, _ = run_loop("fake_fix")
    assert passed and use["calls"] == 2


def test_b2_heldout_against_reference():
    sol, _, _, _, _ = run_loop("fake_ok")
    rows = run_b2.run_heldout(sol)
    assert len(rows) == 200 and all(r["error"] is None and not r["float"] for r in rows)


def test_b2_float_and_crash_are_flagged():
    sol = (CASE / "reference.py").read_text() + "\n_d = decide_loan\ndef decide_loan(a, p, b):\n    r = _d(a, p, b)\n    r['required_monthly_installment'] = float(r['required_monthly_installment'])\n    return r\n"
    assert all(r["float"] for r in run_b2.run_heldout(sol))
    rows = run_b2.run_heldout("def decide_loan(a, p, b):\n    raise ValueError('boom')\n")
    assert all("boom" in r["error"] for r in rows)


def test_velme_parse_run():
    ok = '{"status":"ok","results":[{"status":"ok","result":{"a":1.50}}]}'
    assert run_velme.parse_run(ok, 0) == ({"a": D("1.50")}, None)
    bad = '{"status":"failed","results":[{"status":"failed","diagnostics":[{"code":"VL0702"}]}],"diagnostics":[]}'
    assert run_velme.parse_run(bad, 4) == (None, "exit 4: VL0702")
    assert run_velme.parse_run("oops", 70)[1].startswith("exit 70")
