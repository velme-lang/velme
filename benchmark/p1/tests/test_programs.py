"""Offline tests for the DMN programs added in P1b (0020 vacation days, 0014 loan comparison): descriptors, generators,
references against the TCK's own cases, examples, and the harness pieces they need (list fields, nested TCK values)."""
import ast
import json
import sys
import xml.etree.ElementTree as ET
from argparse import Namespace
from decimal import Decimal as D, localcontext
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent))

import common  # noqa: E402
import oracle  # noqa: E402
import score  # noqa: E402

PROGRAMS = ["0020", "0014"]
Q = D("1e-8")


def tck_cases(program):
    """[(inputs, {result name: expected})] from the TCK file; skipped when ./fetch_tck.sh hasn't run."""
    path = common.program_path(program["id"], program["dmn"]["tests"])
    if not path.exists():  # P1b review finding 1: fail, never skip, so the TCK checks can't silently drop out
        pytest.fail(f"{path} is missing; run ./fetch_tck.sh")
    out = []
    for tc in ET.parse(path).getroot().findall("{*}testCase"):
        inputs = {n.get("name"): oracle.xml_value(n) for n in tc.findall("{*}inputNode")}
        out.append((inputs, {r.get("name"): oracle.xml_value(r.find("{*}expected")) for r in tc.findall("{*}resultNode")}))
    return out


def rounded(v):
    if isinstance(v, list):
        return [rounded(x) for x in v]
    if isinstance(v, dict):
        return {k: rounded(x) for k, x in v.items()}
    return v.quantize(Q) if isinstance(v, D) else v


@pytest.mark.parametrize("pid", PROGRAMS)
def test_descriptor_loads_and_names_its_files(pid):
    prog = common.load_program(pid)
    folder = common.program_dir(pid)
    assert prog["id"] == pid and prog["oracle"] == "drools"
    for name in ("requirements.md", "signatures.py", "examples.json", "gen.py", "oracle.py", "reference.py", prog["source"]):
        assert (folder / name).is_file(), name
    assert f"goal {prog['goal']}(" in (folder / prog["source"]).read_text()
    assert (folder / prog["source"]).read_text().startswith("language: velme/0.2\n")
    defs = {n.name: [a.arg for a in n.args.args] for n in ast.parse((folder / "signatures.py").read_text()).body
            if isinstance(n, ast.FunctionDef)}
    assert defs[prog["entry"]] == prog["inputs"]
    ref = common.load_module(pid, "reference")
    out = getattr(ref, prog["entry"])(*[D(5000) if pid == "0014" else D(30) for _ in prog["inputs"]])
    assert set(out) == set(common.field_names(prog))
    for ex in json.loads((folder / "examples.json").read_text()):
        assert ex["function"] in defs and len(ex["args"]) == len(defs[ex["function"]])


def in_domain_0020(case):
    a, y = case["age"], case["years_of_service"]
    return a == a.to_integral_value() and y == y.to_integral_value() and 16 <= a <= 80 and 0 <= y <= 60


def in_domain_0014(case):
    a = case["requested_amt"]
    return a == a.quantize(D("0.01")) and D(5000) <= a <= D(1500000)


@pytest.mark.parametrize("pid,in_domain", [("0020", in_domain_0020), ("0014", in_domain_0014)])
def test_gen_yields_200_in_domain_cases_with_kinds(pid, in_domain):
    prog = common.load_program(pid)
    cases = common.load_module(pid, "gen").generate(1, 200)
    assert [c["id"] for c in cases] == list(range(1, 201))
    assert all(in_domain(c) for c in cases)
    assert all(set(prog["inputs"]) <= set(c) for c in cases)
    kinds = [c["kind"] for c in cases]
    assert set(kinds) == {"boundary", "random"} and kinds.count("boundary") >= 100
    assert common.dumps(cases) == common.dumps(common.load_module(pid, "gen").generate(1, 200))  # seeded
    assert len({common.dumps({k: c[k] for k in prog["inputs"]}) for c in cases if c["kind"] == "boundary"}) > 100


def test_0020_gen_lands_on_and_next_to_every_threshold():
    cases = common.load_module("0020", "gen").generate(1, 200)
    ages, years = {int(c["age"]) for c in cases}, {int(c["years_of_service"]) for c in cases}
    assert {16, 17, 18, 19, 44, 45, 46, 59, 60, 61, 80} <= ages
    assert {0, 14, 15, 16, 29, 30, 31, 60} <= years


def test_0014_gen_brackets_every_ranking_crossing():
    """Each pair of products swaps places (or ties) in the named ranking between the amounts gen puts either side."""
    gen = common.load_module("0014", "gen")
    ref = common.load_module("0014", "reference")
    key = {"down": "down_pmt_amt", "monthly": "payment_amt", "equity": "equity_36_mo_pct"}
    sides = {}
    for amount, tag in gen.boundary_amounts():
        if tag.startswith("range"):
            continue
        name, pair, side = tag.split(":")
        sides.setdefault((name, pair), {})[side] = amount
    assert len(sides) > 40
    for (name, pair), at in sides.items():
        i, j = map(int, pair.split("-"))
        diff = {s: (lambda t: t[i][key[name]] - t[j][key[name]])(ref.metrics_table(ref.bankrates(), a)) for s, a in at.items()}
        if "eq" in at:
            assert diff["eq"] == 0, (name, pair)
        if "-" in at and "+" in at:
            assert diff["-"] * diff["+"] < 0, (name, pair)
    cases = gen.generate(1, 200)
    assert {c["requested_amt"] for c in cases} >= {D("5000.00"), D("1500000.00")}


def test_0020_reference_matches_the_tck_cases():
    prog = common.load_program("0020")
    ref = common.load_module("0020", "reference")
    cases = tck_cases(prog)
    assert len(cases) == 7
    for inputs, want in cases:
        got = ref.vacation_days(inputs["Age"], inputs["Years of Service"])
        assert got["total_vacation_days"] == want["Total Vacation Days"]


def test_0014_reference_matches_the_tck_case():
    prog = common.load_program("0014")
    ref = common.load_module("0014", "reference")
    adapter = common.load_module("0014", "oracle")
    (inputs, want), = tck_cases(prog)
    got = ref.rank_products(inputs["RequestedAmt"])
    assert rounded(got) == rounded(adapter.from_dmn({"ranked_products": want["RankedProducts"]}))
    assert [{"lenderName": p["lender_name"], "rate": p["rate"], "points": p["points"], "fee": p["fee"]}
            for p in ref.bankrates()] == want["Bankrates"]


@pytest.mark.parametrize("pid", PROGRAMS)
def test_examples_match_the_reference_and_stay_in_domain(pid):
    """0014's payment and equity are shown to 8 places, as requirements.md says; everything else is exact."""
    ref = common.load_module(pid, "reference")
    for ex in common.loads(common.program_path(pid, "examples.json").read_text()):
        got = getattr(ref, ex["function"])(*ex["args"])
        if pid == "0014":
            assert rounded(got) == ex["expected"], ex["function"]
            if ex["function"] == "metrics_table":
                assert in_domain_0014({"requested_amt": ex["args"][1]})
        else:
            assert got == ex["expected"], ex
            assert in_domain_0020({"age": D(ex["args"][0]), "years_of_service": D(ex["args"][1])})


def test_0014_examples_show_a_tie_kept_in_table_order():
    ex = [e for e in common.loads(common.program_path("0014", "examples.json").read_text()) if e["function"] == "rank_metrics"]
    tied = [e for e in ex if len({m["down_pmt_amt"] for m in e["args"][0]}) < len(e["args"][0])]
    assert tied
    table = tied[0]["args"][0]
    down = tied[0]["expected"]["rank_by_down_pmt"]
    a, b = [m for m in table if [x["down_pmt_amt"] for x in table].count(m["down_pmt_amt"]) == 2]
    assert down.index(a) < down.index(b)


def test_score_list_field_compares_order_and_8_places():
    prog = common.load_program("0014")
    ref = common.load_module("0014", "reference")
    want = ref.rank_products(D("330000"))
    near = rounded(want)  # within 8 places of the exact values
    assert score.score_case({"output": near}, want, prog["fields"]) == ([], False)
    swapped = {**near, "rank_by_monthly_pmt": near["rank_by_monthly_pmt"][::-1]}
    assert score.score_case({"output": swapped}, want, prog["fields"])[0] == ["rank_by_monthly_pmt"]
    off = [dict(m) for m in near["metrics_table"]]
    off[0]["loan_amt"] += D("0.000000001")  # exact fields stay exact
    assert score.score_case({"output": {**near, "metrics_table": off}}, want, prog["fields"])[0] == ["metrics_table"]
    short = {**near, "rank_by_down_pmt": near["rank_by_down_pmt"][:9]}
    assert score.score_case({"output": short}, want, prog["fields"])[0] == ["rank_by_down_pmt"]
    assert score.score_case({"output": {**near, "rank_by_equity_pct": "x"}}, want, prog["fields"])[0] == ["rank_by_equity_pct"]


def test_xml_value_reads_a_list_of_records():
    node = ET.fromstring(
        '<expected xmlns="t" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="x"><component name="t"><list>'
        '<item><component name="n"><value xsi:type="xsd:string">A</value></component>'
        '<component name="r"><value xsi:type="xsd:decimal">.035</value></component></item>'
        '<item><component name="n"><value xsi:type="xsd:string">B</value></component>'
        '<component name="r"><value xsi:nil="true"/></component></item></list></component></expected>')
    got = oracle.xml_value(node)
    assert got == {"t": [{"n": "A", "r": D("0.035")}, {"n": "B", "r": None}]}
    assert oracle.has_null(got) and not oracle.has_null({"t": [{"n": "A"}]})


def test_0014_adapter_reshapes_dmn_and_cross_checks(monkeypatch):
    prog = common.load_program("0014")
    adapter = common.load_module("0014", "oracle")
    ref = common.load_module("0014", "reference")
    keys = {v: k for k, v in adapter.KEYS.items()}
    exact = ref.rank_products(D("6200.00"))
    dmn = {k: [{keys[f]: x for f, x in m.items()} for m in exact[v]] for k, v in adapter.LISTS.items()}
    monkeypatch.setattr(oracle, "evaluate", lambda model, context: {"RankedProducts": dmn, "RequestedAmt": context["RequestedAmt"]})
    case = {"id": 7, "requested_amt": D("6200.00")}
    row = oracle.dmn_row(prog, case, "model", adapter)
    assert set(row) == {"id", *common.field_names(prog)}
    assert adapter.check(case, row) is None
    down = row["rank_by_down_pmt"]
    k = next(k for k in range(9) if down[k]["down_pmt_amt"] == down[k + 1]["down_pmt_amt"])  # 6200.00 is a tie
    swapped = [*down[:k], down[k + 1], down[k], *down[k + 2:]]
    assert adapter.check(case, {**row, "rank_by_down_pmt": swapped})
    assert adapter.tck_equal("x", {"a": [D("1481.847469769120902")]}, {"a": [D("1481.847469769075")]})
    assert not adapter.tck_equal("x", {"a": [D("1481.8474")]}, {"a": [D("1481.847469769075")]})


def test_selftest_uses_the_adapters_tck_equal(monkeypatch, tmp_path):
    tests = tmp_path / "t.xml"
    tests.write_text('<testCases xmlns="t" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="x">'
                     '<testCase id="1"><inputNode name="A"><value xsi:type="xsd:decimal">1</value></inputNode>'
                     '<resultNode name="R"><expected><value xsi:type="xsd:decimal">0.1234567891</value></expected>'
                     '</resultNode></testCase></testCases>')
    monkeypatch.setattr(oracle, "program_path", lambda program, relative: tests)
    monkeypatch.setattr(oracle, "evaluate", lambda model, context: {"R": D("0.1234567890999")})
    monkeypatch.setattr(oracle, "load_module", lambda program, name: Namespace())
    assert not oracle.selftest({"id": "fake", "dmn": {"model": "m", "tests": "t"}})  # exact by default
    adapter = Namespace(tck_equal=lambda name, got, want: rounded(got) == rounded(want))
    monkeypatch.setattr(oracle, "load_module", lambda program, name: adapter)
    assert oracle.selftest({"id": "fake", "dmn": {"model": "m", "tests": "t"}})


def test_each_program_loads_its_own_reference():
    """Finding: a second program's adapter imported the first one's `reference` (same module name)."""
    first = common.load_module("0020", "oracle")
    second = common.load_module("0014", "oracle")
    assert hasattr(first.reference, "vacation_days") and hasattr(second.reference, "rank_products")


def test_0014_gen_keeps_every_boundary_amount():
    """P1b review finding 2: every boundary amount is among the 200 generated cases."""
    gen = common.load_module("0014", "gen")
    assert {a for a, _ in gen.boundary_amounts()} <= {c["requested_amt"] for c in gen.generate(1, 200)}


def test_0014_every_crossing_is_bracketed_by_generated_amounts():
    """P1b review finding 3: each crossing in each ranking, whatever tag its amount carries, has a generated amount on
    it (a tie) or one on each side within a cent."""
    gen = common.load_module("0014", "gen")
    amounts = sorted({c["requested_amt"] for c in gen.generate(1, 200)})
    count = 0
    with localcontext() as ctx:
        ctx.prec = 50
        for name, ls in gen.lines().items():
            for i in range(len(ls)):
                for j in range(i + 1, len(ls)):
                    (ai, bi), (aj, bj) = ls[i], ls[j]
                    if ai == aj or not gen.LO <= (x := (bj - bi) / (ai - aj)) <= gen.HI:
                        continue
                    count += 1
                    below = [a for a in amounts if x - gen.CENT <= a < x]
                    above = [a for a in amounts if x < a <= x + gen.CENT]
                    assert x in amounts or (below and above), (name, i, j, x)
    assert count > len({a for a, _ in gen.boundary_amounts()}) // 3
