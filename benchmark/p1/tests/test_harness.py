"""Offline tests for the per-program harness: scoring, prompts, arms, kinds and pooling."""
import json
import sys
from argparse import Namespace
from decimal import Decimal as D
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent))

import common  # noqa: E402
import gen_inputs  # noqa: E402
import oracle  # noqa: E402
import run_b1  # noqa: E402
import run_velme  # noqa: E402
import run_b2  # noqa: E402
import score  # noqa: E402

PROGRAM = common.load_program("0087")
MODEL = "claude-sonnet-5-5"
FIXTURES = Path(__file__).parent / "fixtures"

WANT = common.loads(json.dumps({
    "application_risk_score": 138, "pre_bureau_risk_category": "VERY LOW", "required_monthly_installment": "x",
    "pre_bureau_affordability": True, "eligibility": "ELIGIBLE", "bureau_call_type": "NONE", "strategy": "THROUGH",
    "post_bureau_risk_category": "VERY LOW", "post_bureau_affordability": True, "routing": "ACCEPT"}).replace('"x"', "20.123456789012345678"))


def case(**over):
    return {"output": {**WANT, **over}}


def toy(monkeypatch):
    monkeypatch.setattr(common, "HERE", FIXTURES)


def test_pass():
    assert score.score_case(case(), WANT, PROGRAM["fields"]) == ([], False)


def test_wrong_field():
    assert score.score_case(case(routing="REFER"), WANT, PROGRAM["fields"])[0] == ["routing"]
    assert score.score_case(case(pre_bureau_affordability=1), WANT, PROGRAM["fields"])[0] == ["pre_bureau_affordability"]
    assert score.score_case(case(application_risk_score="138"), WANT, PROGRAM["fields"])[0] == ["application_risk_score"]


def test_float_counts():
    row = {**case(), "float": True}
    assert score.score_case(row, WANT, PROGRAM["fields"]) == ([], True)


def test_installment_precision():
    assert score.score_case(case(required_monthly_installment=D("20.1234567891")), WANT, PROGRAM["fields"])[0] == []
    assert score.score_case(case(required_monthly_installment=D("20.123457")), WANT, PROGRAM["fields"])[0] == ["required_monthly_installment"]
    assert score.score_case({"output": None}, WANT, PROGRAM["fields"])[0] == common.field_names(PROGRAM)


def test_trial_scoring(tmp_path):
    expected = {1: WANT, 2: WANT}
    (tmp_path / "results.jsonl").write_text(common.dumps({"id": 1, **case(), "error": None}) + "\n" + common.dumps({"id": 2, "output": None, "error": "x"}) + "\n")
    r = score.score_trial(PROGRAM, tmp_path, expected, {1: "boundary", 2: "random"})
    assert (r["passed"], r["b_pass"], r["r_pass"], r["no_output"]) == (1, 1, 0, 1)


def test_cost_cached_and_uncached():
    m = {"tokens_in": 1_000_000, "tokens_out": 1_000_000, "cache_read": 1_000_000, "cache_write": 1_000_000}
    assert abs(common.cost(m, MODEL) - (2 + 10 + 0.2 + 2.5)) < 1e-9
    assert abs(common.cost_uncached(m, MODEL) - ((1 + 1 + 1) * 2 + 10)) < 1e-9


def b2_args(name):
    return Namespace(model="fake", fake_reply=str(Path(__file__).parent / name), no_prompt_cache=False)


def run_loop(name):
    return run_b2.model_loop(b2_args(name), PROGRAM, run_b2.build_system(PROGRAM, True))


def test_b2_first_reply_passes():
    _, _, passed, use, _ = run_loop("fake_ok")
    assert passed and use["calls"] == 1


def test_b2_fix_round():
    _, _, passed, use, _ = run_loop("fake_fix")
    assert passed and use["calls"] == 2


def test_b2_heldout_against_reference():
    sol, _, _, _, _ = run_loop("fake_ok")
    rows = run_b2.run_heldout(PROGRAM, sol)
    assert len(rows) == 200 and all(r["error"] is None and not r["float"] for r in rows)


def test_b2_float_and_crash_are_flagged():
    sol = (common.program_path("0087", "reference.py")).read_text() + "\n_d = decide_loan\ndef decide_loan(a, p, b):\n    r = _d(a, p, b)\n    r['required_monthly_installment'] = float(r['required_monthly_installment'])\n    return r\n"
    assert all(r["float"] for r in run_b2.run_heldout(PROGRAM, sol))
    rows = run_b2.run_heldout(PROGRAM, "def decide_loan(a, p, b):\n    raise ValueError('boom')\n")
    assert all("boom" in r["error"] for r in rows)


def test_b1_prompt_is_b2_prompt_without_the_tests_instruction():
    b2 = run_b2.build_system(PROGRAM, True)
    b1 = run_b2.build_system(PROGRAM, False)
    assert b1 == b2.replace(run_b2.TESTS_INSTRUCTION, run_b2.SOLUTION_INSTRUCTION)
    assert "pytest" not in b1 and "test_solution" not in b1
    assert "pytest" in b2 and "test_solution" in b2


def test_cache_control_in_b1_and_b2_requests():
    payload = run_b2.request_payload(MODEL, "system", [run_b2.first_message(True)], True)
    assert payload["system"][0]["cache_control"] == {"type": "ephemeral"}
    off = run_b2.request_payload(MODEL, "system", [run_b2.first_message(True)], False)
    assert "cache_control" not in json.dumps(off)
    assert run_b1.call_api is run_b2.call_api  # B1 sends the same cached system prefix


def test_call_api_transport_sends_cache_control(monkeypatch):
    class Reply:
        def read(self):
            return b'{"content": [], "usage": {}}'

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

    seen = {}

    def urlopen(request, timeout=None):
        seen["body"] = json.loads(request.data)
        return Reply()

    monkeypatch.setattr(run_b2.urllib.request, "urlopen", urlopen)
    monkeypatch.setenv("ANTHROPIC_API_KEY", "test-key")
    run_b2.call_api(MODEL, "system", [run_b2.first_message(True)], True)
    assert seen["body"]["system"][0]["cache_control"] == {"type": "ephemeral"}


def test_b1_is_one_call_on_a_second_program(monkeypatch, tmp_path):
    toy(monkeypatch)
    program = common.load_program("toy")
    args = Namespace(model="fake", fake_reply=str(Path(__file__).parent / "fake_toy"), no_prompt_cache=False,
                     out=str(tmp_path))
    rows, meta, solution, _ = run_b1.trial(args, program, 1, run_b2.build_system(program, False))
    assert meta["calls"] == 1 and meta["build_ok"]
    assert len(rows) == 4 and all(r["error"] is None for r in rows) and solution


def test_b1_trial_passes_cache_flag_and_really_calls_once(monkeypatch, tmp_path):
    """Finding: the fake-reply path hard-coded calls=1, so nothing proved the B1 call or its cache flag."""
    toy(monkeypatch)
    program = common.load_program("toy")
    text = (Path(__file__).parent / "fake_toy" / "reply-1.txt").read_text()
    seen = []

    def fake_call(model, system, messages, cache):
        seen.append(cache)
        return {"content": [{"type": "text", "text": text}],
                "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 3, "cache_creation_input_tokens": 2}}

    monkeypatch.setattr(run_b1, "call_api", fake_call)
    monkeypatch.setattr(run_b1, "imports_ok", lambda solution: True)
    monkeypatch.setattr(run_b1, "run_heldout", lambda program, solution: [])
    args = Namespace(model=MODEL, fake_reply=None, no_prompt_cache=False, out=str(tmp_path))
    _, meta, _, _ = run_b1.trial(args, program, 1, run_b2.build_system(program, False))
    assert seen == [True]
    assert (meta["calls"], meta["tokens_in"], meta["cache_read"], meta["cache_write"]) == (1, 10, 3, 2)
    args.no_prompt_cache = True
    run_b1.trial(args, program, 1, run_b2.build_system(program, False))
    assert seen == [True, False]


def test_b2_model_loop_passes_cache_flag_and_really_calls_once(monkeypatch):
    """Finding: cache_control was only tested on the payload, not on what model_loop sends."""
    toy(monkeypatch)
    program = common.load_program("toy")
    text = "```python\nx = 1\n```\n```python\ndef test_x(): assert True\n```"
    seen = []

    def fake_call(model, system, messages, cache):
        seen.append(cache)
        return {"content": [{"type": "text", "text": text}], "usage": {"input_tokens": 4, "output_tokens": 2}}

    monkeypatch.setattr(run_b2, "call_api", fake_call)
    monkeypatch.setattr(run_b2, "run_pytest", lambda solution, tests: (0, "ok"))
    args = Namespace(model=MODEL, fake_reply=None, no_prompt_cache=False)
    _, _, passed, use, _ = run_b2.model_loop(args, program, run_b2.build_system(program, True))
    assert passed and use["calls"] == 1 and seen == [True]
    args.no_prompt_cache = True
    run_b2.model_loop(args, program, run_b2.build_system(program, True))
    assert seen == [True, False]


def test_read_synth_log_reads_cache_tokens(tmp_path):
    """Finding: nothing tested that run_velme reads cache_read_tokens/cache_write_tokens from the synth log."""
    project = tmp_path / "project"
    (project / ".velme").mkdir(parents=True)
    lines = [
        {"tokens_in": 10, "tokens_out": 5, "cache_read_tokens": 7, "cache_write_tokens": 3, "outcome": "ok", "attempt": 0},
        {"tokens_in": 2, "tokens_out": 1, "cache_read_tokens": None, "cache_write_tokens": 4, "outcome": "VL0401", "attempt": 1},
    ]
    (project / ".velme" / "synth-log.jsonl").write_text("".join(json.dumps(line) + "\n" for line in lines))
    got = run_velme.read_synth_log(project)
    assert (got["tokens_in"], got["tokens_out"], got["cache_read"], got["cache_write"], got["calls"]) == (12, 6, 7, 7, 2)


def test_runner_write_path_scores_with_discover(monkeypatch, tmp_path):
    """Finding: write_trial added trial-N a second time, so no real runner's output could be scored."""
    toy(monkeypatch)
    root = tmp_path / "runs"
    monkeypatch.setattr(sys, "argv", [
        "run_b1.py", "--program", "toy", "--model", MODEL, "--trials", "1", "--out", str(root),
        "--fake-reply", str(Path(__file__).parent / "fake_toy")])
    run_b1.main()
    trial = root / "toy" / "b1" / MODEL / "trial-1"
    assert (trial / "meta.json").exists() and not (trial / "trial-1").exists()
    records = score.discover(root)
    assert len(records) == 1 and records[0]["score"]["total"] == 4


def test_b1_wall_stops_before_heldout(monkeypatch, tmp_path):
    """Finding: B1 took wall_s after imports_ok and run_heldout, unlike B2 and Velme."""
    toy(monkeypatch)
    program = common.load_program("toy")
    ticks = iter([0.0, 1.0, 9.0])
    monkeypatch.setattr(run_b1.time, "monotonic", lambda: next(ticks))
    monkeypatch.setattr(run_b1, "imports_ok", lambda solution: True)
    monkeypatch.setattr(run_b1, "run_heldout", lambda program, solution: [])
    args = Namespace(model="fake", fake_reply=str(Path(__file__).parent / "fake_toy"), no_prompt_cache=False,
                     out=str(tmp_path))
    _, meta, _, _ = run_b1.trial(args, program, 1, run_b2.build_system(program, False))
    assert meta["wall_s"] == 1.0


def test_toy_program_is_not_0087_bound(monkeypatch):
    toy(monkeypatch)
    program = common.load_program("toy")
    assert common.field_names(program) == ["doubled", "half"]
    inputs, kinds, expected = common.load_inputs("toy"), common.load_kinds("toy"), common.load_expected("toy")
    assert [c["id"] for c in inputs] == [1, 2, 3, 4]
    assert kinds == {1: "boundary", 2: "boundary", 3: "boundary", 4: "random"}
    assert len(expected) == 4
    assert common.load_module("toy", "oracle").decide(inputs[0]) == {"doubled": D(2), "half": D("0.5")}
    assert "Toy rules" in run_b2.build_system(program, True)


def test_python_oracle_writes_expected(monkeypatch, tmp_path):
    toy(monkeypatch)
    program = common.load_program("toy")
    out = tmp_path / "expected.jsonl"
    monkeypatch.setattr(oracle, "heldout_path", lambda p, name: out)
    oracle.run_all(program)
    assert common.read_jsonl(out) == common.read_jsonl(FIXTURES / "toy" / "heldout" / "expected.jsonl")


def test_python_oracle_cross_checks_reference_py(monkeypatch, tmp_path, capsys):
    """A Python-oracle program with a reference.py gets the same "reference differs on N" line as the Drools path; the
    toy reference differs from the toy oracle on case 4 (value 25) only. A program without one reports 0."""
    toy(monkeypatch)
    monkeypatch.setattr(oracle, "heldout_path", lambda p, name: tmp_path / f"{p}-{name}")
    oracle.run_all(common.load_program("toy"))
    out = capsys.readouterr().out
    assert "reference differs on 1" in out
    assert "case 4 " in out and "reference and oracle differ" in out and not any(f"case {i} " in out for i in (1, 2, 3))
    assert common.read_jsonl(tmp_path / "toy-expected.jsonl")[3]["doubled"] == 50  # the oracle's value is written
    oracle.run_all(common.load_program("toy2"))
    assert "reference differs on 0" in capsys.readouterr().out


def test_reference_diff_uses_each_fields_compare_mode(monkeypatch):
    """Fix round 1, finding 1: `half` compares to 2 places, so a reference differing only past them isn't counted."""
    toy(monkeypatch)
    program = common.load_program("toy")
    case, row = {"id": 1, "value": D(9)}, {"id": 1, "doubled": D(18), "half": D("4.5")}
    ref = lambda half: Namespace(decide=lambda value: {"doubled": D(18), "half": half})
    assert oracle.reference_diff(program, case, row, ref(D("4.501"))) is None
    assert oracle.reference_diff(program, case, row, ref(D("4.51")))
    assert oracle.reference_diff(program, case, row, Namespace(decide=lambda value: {"doubled": D(18)}))  # field missing


def test_oracle_selftest_applies_adapter_context_defaults(monkeypatch, tmp_path):
    """Finding: selftest stopped merging 0087's `Supporting documents` default into TCK inputs."""
    class Value:
        text = "1"

        def get(self, name, default=None):
            return ""

    class Node:
        def get(self, name):
            return "A" if name == "name" else "c1"

        def findall(self, path):
            return []

        def find(self, path):
            return Value()

    class Result:
        def get(self, name):
            return "R"

        def find(self, path):
            return Node()

    class Case:
        def get(self, name):
            return "c1"

        def findall(self, path):
            return [Node()] if path.endswith("inputNode") else [Result()]

    class Tree:
        def getroot(self):
            return self

        def findall(self, path):
            return [Case()]

    (tmp_path / "model.xml").write_text("model")
    monkeypatch.setattr(oracle, "program_path", lambda program, relative: tmp_path / "model.xml")
    monkeypatch.setattr(oracle.ET, "parse", lambda path: Tree())
    monkeypatch.setattr(oracle, "load_module", lambda program, name: Namespace(context_defaults={"D": ""}))
    seen = {}

    def fake_evaluate(model, context):
        seen.update(context)
        return {"R": D(1)}

    monkeypatch.setattr(oracle, "evaluate", fake_evaluate)
    assert oracle.selftest({"id": "fake", "dmn": {"model": "m", "tests": "t"}})
    assert seen == {"D": "", "A": D(1)}


def test_0087_generator_keeps_committed_inputs_and_kinds_align(tmp_path):
    out = tmp_path / "inputs.jsonl"
    gen_inputs.write("0087", 1, 200, out)
    assert out.read_bytes() == common.heldout_path("0087", "inputs.jsonl").read_bytes()
    generated = {r["id"]: r["kind"] for r in common.read_jsonl(tmp_path / "kinds.jsonl")}
    assert generated == common.load_kinds("0087")
    assert set(generated) == {c["id"] for c in common.load_inputs("0087")}
    assert set(generated.values()) <= {"boundary", "random"}


def test_kind_is_validated_and_only_0087_may_derive_it(monkeypatch, tmp_path):
    """Finding: an unknown kind was silently scored as random, and non-0087 programs could lack one."""
    assert gen_inputs.kind_of({"tags": ["random"]}, "0087") == "random"
    assert gen_inputs.kind_of({"kind": "boundary"}, "toy") == "boundary"
    with pytest.raises(ValueError):
        gen_inputs.kind_of({"kind": "Random"}, "toy")
    with pytest.raises(ValueError):
        gen_inputs.kind_of({}, "toy")

    class BadGen:
        @staticmethod
        def generate(seed, n):
            return [{"id": 1, "value": 1, "kind": "edge"}]

    monkeypatch.setattr(gen_inputs, "load_module", lambda program, name: BadGen)
    out = tmp_path / "inputs.jsonl"
    with pytest.raises(ValueError):
        gen_inputs.write("toy", 1, 1, out)
    assert not out.exists() and not (tmp_path / "kinds.jsonl").exists()  # fail before writing

    monkeypatch.setattr(common, "heldout_path", lambda program, name: tmp_path / name)
    with pytest.raises(ValueError):
        common.load_kinds("toy")  # toy is not 0087, so it must carry kinds.jsonl
    (tmp_path / "kinds.jsonl").write_text('{"id": 1, "kind": "edge"}\n')
    with pytest.raises(ValueError):
        common.load_kinds("toy")
    with pytest.raises(ValueError):
        score.score_trial(PROGRAM, tmp_path / "missing", {1: WANT}, {1: "edge"})


def test_runners_reject_an_unpriced_model_before_the_run(monkeypatch, tmp_path):
    """Finding: an unpriced model only failed in score.py, after the paid trials had run."""
    for module, name in ((run_b1, "run_b1.py"), (run_b2, "run_b2.py"), (run_velme, "run_velme.py")):
        monkeypatch.setattr(sys, "argv", [name, "--program", "0087", "--model", "unpriced", "--trials", "1",
                                          "--out", str(tmp_path)])
        with pytest.raises(SystemExit):
            module.main()


def test_fake_reply_runner_skips_the_price_check(monkeypatch, tmp_path):
    toy(monkeypatch)
    root = tmp_path / "runs"
    monkeypatch.setattr(sys, "argv", [
        "run_b1.py", "--program", "toy", "--model", "unpriced", "--trials", "1", "--out", str(root),
        "--fake-reply", str(Path(__file__).parent / "fake_toy")])
    run_b1.main()
    assert (root / "toy" / "b1" / "unpriced" / "trial-1" / "meta.json").exists()


def write_trial(root, program, arm, trial, outputs, meta=None, floats=()):
    d = common.trial_dir(root, program, arm, MODEL, trial)
    d.mkdir(parents=True, exist_ok=True)
    rows = [{"id": id_, "output": {k: v for k, v in out.items() if k != "id"}, "error": None, "float": id_ in floats}
            for id_, out in outputs.items()]
    (d / "results.jsonl").write_text("".join(common.dumps(r) + "\n" for r in rows))
    (d / "meta.json").write_text(json.dumps(meta or {
        "program": program, "arm": arm, "model": MODEL, "trial": trial, "build_ok": True,
        "tokens_in": 0, "tokens_out": 0, "cache_read": 0, "cache_write": 0, "calls": 1, "wall_s": 0}))


def test_scoring_pools_two_programs_and_counts_a_failed_velme_build(monkeypatch, tmp_path):
    toy(monkeypatch)
    root = tmp_path / "runs"
    want_toy, want_toy2 = common.load_expected("toy"), common.load_expected("toy2")
    broken = dict(want_toy)
    broken[1] = {**want_toy[1], "doubled": D(999)}  # a boundary failure
    write_trial(root, "toy", "velme", 1, want_toy)
    write_trial(root, "toy2", "velme", 1, want_toy2)
    write_trial(root, "toy", "b2", 1, broken, floats={4})  # a float makes its random case fail too
    write_trial(root, "toy2", "b2", 1, want_toy2)
    write_trial(root, "toy", "velme", 2, {}, meta={
        "program": "toy", "arm": "velme", "model": MODEL, "trial": 2, "build_ok": False,
        "tokens_in": 0, "tokens_out": 0, "cache_read": 0, "cache_write": 0, "calls": 1, "wall_s": 0})

    records = score.discover(root)
    assert len(records) == 5
    pooled = score.group(records, lambda r: (r["model"], r["arm"]))
    velme, b2 = pooled[(MODEL, "velme")], pooled[(MODEL, "b2")]
    assert (velme["passed"], velme["total"]) == (6, 10)  # failed build is 0/4, not a missing trial
    assert velme["builds_failed"] == 1
    assert (b2["passed"], b2["total"], b2["float"]) == (4, 6, 1)
    assert (b2["b_pass"], b2["b_total"], b2["r_pass"], b2["r_total"]) == (3, 4, 1, 2)
    coverage = {}
    for record in records:
        coverage.setdefault((record["model"], record["arm"]), set()).add((record["program"], record["trial"]))
    assert any("INCOMPLETE" in line and "B2 missing toy/trial-2" in line for line in score.go_lines(pooled, coverage))


def test_go_line_requires_matching_program_and_trial_coverage():
    """Finding: the go line compared pooled totals from arms covering different programs/trials."""
    pooled = {
        (MODEL, "velme"): {"passed": 10, "total": 10, "b_pass": 10, "b_total": 10, "cost": 1.0},
        (MODEL, "b2"): {"passed": 8, "total": 10, "b_pass": 8, "b_total": 10, "cost": 1.0},
    }
    same = {(MODEL, "velme"): {("toy", "trial-1")}, (MODEL, "b2"): {("toy", "trial-1")}}
    assert "GO" in score.go_lines(pooled, same)[0]
    extra = {(MODEL, "velme"): {("toy", "trial-1"), ("toy2", "trial-1")},
             (MODEL, "b2"): {("toy", "trial-1")}}
    line = score.go_lines(pooled, extra)[0]
    assert "INCOMPLETE" in line and "B2 missing toy2/trial-1" in line


def test_velme_parse_run():
    ok = '{"status":"ok","results":[{"status":"ok","result":{"a":1.50}}]}'
    assert run_velme.parse_run(ok, 0) == ({"a": D("1.50")}, None)
    bad = '{"status":"failed","results":[{"status":"failed","diagnostics":[{"code":"VL0702"}]}],"diagnostics":[]}'
    assert run_velme.parse_run(bad, 4) == (None, "exit 4: VL0702")
    assert run_velme.parse_run("oops", 70)[1].startswith("exit 70")


def test_synth_log_meta_is_json(tmp_path):
    """P1e smoke: token counts read with the Decimal loader made meta.json fail to serialize."""
    log = tmp_path / ".velme/synth-log.jsonl"
    log.parent.mkdir()
    log.write_text('{"tokens_in": 10, "tokens_out": 5, "cache_read_tokens": 100, "outcome": "ok", "attempt": 0}\n')
    meta = run_velme.read_synth_log(tmp_path)
    json.dumps(meta)
    assert (meta["tokens_in"], meta["cache_read"], meta["calls"]) == (10, 100, 1)


def test_decimals_compare_fails_on_a_huge_value_instead_of_crashing():
    """Finding: quantize raised InvalidOperation on 1e25 at 8 places and killed the whole scoring run."""
    field = {"name": "x", "compare": "decimals", "decimals": 8}
    assert not score.field_ok(field, D("1e25"), D("1e25"))
    assert score.field_ok(field, D("1.5"), D("1.5"))


def test_discover_skips_a_trial_without_meta_and_coverage_marks_it_incomplete(monkeypatch, tmp_path):
    """Finding: an interrupted run leaves trial-N/build/ with no meta.json, and discover died on it."""
    toy(monkeypatch)
    root = tmp_path / "runs"
    want = common.load_expected("toy")
    for arm in ("velme", "b2"):
        write_trial(root, "toy", arm, 1, want)
    (common.trial_dir(root, "toy", "velme", MODEL, 2) / "build").mkdir(parents=True)
    records = score.discover(root)
    assert sorted((r["arm"], r["trial"]) for r in records) == [("b2", "trial-1"), ("velme", "trial-1")]
    pooled = score.group(records, lambda r: (r["model"], r["arm"]))
    coverage = {}
    for r in records:
        coverage.setdefault((r["model"], r["arm"]), set()).add((r["program"], r["trial"]))
    coverage[(MODEL, "velme")].discard(("toy", "trial-2"))
    coverage[(MODEL, "b2")].add(("toy", "trial-2"))  # the other arm did run trial 2
    assert any("INCOMPLETE" in line for line in score.go_lines(pooled, coverage))


def test_report_exits_on_a_trial_present_in_two_roots(monkeypatch, tmp_path):
    """Finding: report concatenated records from several roots, counting a repeated trial twice."""
    toy(monkeypatch)
    want = common.load_expected("toy")
    a, b = tmp_path / "a", tmp_path / "b"
    write_trial(a, "toy", "velme", 1, want)
    write_trial(b, "toy", "velme", 2, want)
    assert "# P1 score" in score.report([a, b])  # disjoint trials pool fine
    write_trial(b, "toy", "velme", 1, want)
    with pytest.raises(SystemExit, match="duplicate trial toy/velme/.*/trial-1"):
        score.report([a, b])
