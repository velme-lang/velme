"""Oracle: the original DMN model evaluated by Drools in the Kogito JIT runner container.

  python oracle.py --start       docker run the container and wait until it answers (needs docker; not run by tests)
  python oracle.py --stop
  python oracle.py --selftest    run the TCK's own test cases for model 0087 through the container
  python oracle.py               evaluate 0087/heldout/inputs.jsonl, write 0087/heldout/expected.jsonl

Endpoint shape, from the Kogito sources (apache/incubator-kie-kogito-apps,
jitexecutor/jitexecutor-dmn/.../api/JITDMNResource.java and requests/JITDMNPayload.java):
  POST /jitdmn, Content-Type application/json, body {"model": "<dmn xml as a string>", "context": {<input data name>: value}}
  -> 200 and ONE FLAT JSON OBJECT: every input data name and every decision name -> its value (no decisionResults
  there; POST /jitdmn/dmnresult returns the full DMNResult with decisionResults and messages instead).
Numbers in `context` are parsed by Jackson into Double and turned back into BigDecimal by Drools through
Double.toString (NumberEvalHelper.getBigDecimalOrNull), so decimals with <= 15 significant digits arrive exactly.
"""
import argparse
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
from decimal import Decimal
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "0087"))
from common import dumps, loads  # noqa: E402
import reference  # noqa: E402

URL = "http://localhost:8080/jitdmn"
IMAGE = "quay.io/kiegroup/kogito-jit-runner:latest"
NAME = "velme-p1-kogito"
TCK = HERE / ".cache/tck/TestCases/compliance-level-3/0087-chapter-11-example"
HELDOUT = HERE / "0087/heldout"
FIELDS = {
    "Application risk score": "application_risk_score", "Pre-bureau risk category": "pre_bureau_risk_category",
    "Required monthly installment": "required_monthly_installment", "Pre-bureau affordability": "pre_bureau_affordability",
    "Eligibility": "eligibility", "Bureau call type": "bureau_call_type", "Strategy": "strategy",
    "Post-bureau risk category": "post_bureau_risk_category", "Post-bureau affordability": "post_bureau_affordability",
    "Routing": "routing",
}


def evaluate(model_xml, context):
    # The one input the model declares and no TCK case supplies; only the Adjudication decision (which has no logic) uses it.
    context = {"Supporting documents": "", **context}
    body = '{"model": %s, "context": %s}' % (json.dumps(model_xml), dumps(context))
    req = urllib.request.Request(URL, body.encode(), {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return loads(r.read())


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


def xml_value(node):
    """A TCK <value>/<component> element as a Python value; None when it is nil or absent."""
    comps = node.findall("{*}component")
    if comps:
        return {c.get("name"): xml_value(c) for c in comps}
    v = node.find("{*}value")
    if v is None or v.get("{http://www.w3.org/2001/XMLSchema-instance}nil") == "true":
        return None
    t = v.get("{http://www.w3.org/2001/XMLSchema-instance}type", "")
    if t.endswith("boolean"):
        return v.text.strip() == "true"
    if t.endswith("string"):
        return v.text or ""
    return Decimal(v.text.strip())


def has_null(v):
    return v is None or (isinstance(v, dict) and any(has_null(x) for x in v.values()))


def selftest():
    model = (TCK / "0087-chapter-11-example.dmn").read_text()
    bad = ran = 0
    for tc in ET.parse(TCK / "0087-chapter-11-example-test-01.xml").getroot().findall("{*}testCase"):
        inputs = {n.get("name"): xml_value(n) for n in tc.findall("{*}inputNode")}
        if has_null(inputs):
            print(f"case {tc.get('id')}: skipped (null input)")
            continue
        got = evaluate(model, inputs)
        ran += 1
        for rn in tc.findall("{*}resultNode"):
            want = xml_value(rn.find("{*}expected"))
            ok = got.get(rn.get("name")) == want
            bad += not ok
            print(f"case {tc.get('id')}: {rn.get('name')} = {got.get(rn.get('name'))!r} expected {want!r} {'ok' if ok else 'MISMATCH'}")
    print(f"selftest: {ran} cases run, {bad} mismatches")
    return bad == 0


def run_all():
    model = (TCK / "0087-chapter-11-example.dmn").read_text()
    cases = [loads(l) for l in (HELDOUT / "inputs.jsonl").read_text().splitlines()]
    out, diffs = [], 0
    for c in cases:
        got = evaluate(model, to_context(c))
        missing = [k for k in FIELDS if got.get(k) is None]
        if missing:
            sys.exit(f"case {c['id']}: oracle returned no value for {missing}: {got}")
        row = {"id": c["id"], **{FIELDS[k]: got[k] for k in FIELDS}}
        out.append(row)
        # sanity: requirements.md (the reference implementation) vs the DMN; installment at 8 places, see score.py
        want = reference.decide_loan(c["applicant"], c["product"], c["bureau"])
        q = lambda d: d["required_monthly_installment"].quantize(Decimal("1e-8"))  # noqa: E731
        if {**want, "required_monthly_installment": q(want)} != {**{k: row[k] for k in want}, "required_monthly_installment": q(row)}:
            diffs += 1
            print(f"case {c['id']} {c['tags']}: reference and DMN differ:\n  dmn {row}\n  ref {want}")
    (HELDOUT / "expected.jsonl").write_text("".join(dumps(r) + "\n" for r in out))
    print(f"wrote {len(out)} rows to {HELDOUT / 'expected.jsonl'}; reference differs from DMN on {diffs}")


def start():
    subprocess.run(["docker", "run", "-d", "--rm", "--name", NAME, "-p", "8080:8080", IMAGE], check=True)
    for _ in range(120):
        try:
            urllib.request.urlopen("http://localhost:8080/", timeout=2)
        except urllib.error.HTTPError:
            break  # any HTTP answer means the server is up
        except OSError:
            time.sleep(1)
            continue
        break
    else:
        sys.exit("container did not become ready in 120 s; see `docker logs " + NAME + "`")
    print("ready on http://localhost:8080")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--start", action="store_true")
    ap.add_argument("--stop", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.start:
        start()
    elif args.stop:
        subprocess.run(["docker", "stop", NAME], check=True)
    elif args.selftest:
        sys.exit(0 if selftest() else 1)
    else:
        run_all()
