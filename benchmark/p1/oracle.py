"""Oracle: expected outputs from a reference neither arm sees.

  python oracle.py --program ID --start       docker run the Kogito JIT runner and wait until it answers (DMN only)
  python oracle.py --program ID --stop
  python oracle.py --program ID --selftest    the TCK's own test cases through the container (DMN only)
  python oracle.py --program ID               evaluate heldout/inputs.jsonl, write heldout/expected.jsonl

A DMN program uses Drools over HTTP; its descriptor names the model, the TCK tests and the decision names. Any other
program loads `<program>/oracle.py` and calls its `decide(case)`, which returns a dict of the descriptor's fields.
A DMN program's `<program>/oracle.py` adapter gives `to_context(case)` and optionally `context_defaults`,
`check(case, row)` (cross-check against reference.py), `from_dmn(values)` (reshape a nested result into the fields)
and `tck_equal(name, got, want)` (how close a TCK expected value must be; default exact).
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

from common import dumps, field_names, heldout_path, load_inputs, load_module, load_program, loads, program_path

URL = "http://localhost:8080/jitdmn"
IMAGE = "quay.io/kiegroup/kogito-jit-runner:latest"
NAME = "velme-p1-kogito"


def evaluate(model_xml, context):
    body = '{"model": %s, "context": %s}' % (json.dumps(model_xml), dumps(context))
    req = urllib.request.Request(URL, body.encode(), {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return loads(r.read())


def xml_value(node):
    """A TCK <value>/<component>/<list> element as a Python value; None when it is nil or absent."""
    comps = node.findall("{*}component")
    if comps:
        return {c.get("name"): xml_value(c) for c in comps}
    v = node.find("{*}value")
    if v is None and (items := node.find("{*}list")) is not None:
        return [xml_value(i) for i in items.findall("{*}item")]
    if v is None or v.get("{http://www.w3.org/2001/XMLSchema-instance}nil") == "true":
        return None
    t = v.get("{http://www.w3.org/2001/XMLSchema-instance}type", "")
    if t.endswith("boolean"):
        return v.text.strip() == "true"
    if t.endswith("string"):
        return v.text or ""
    return Decimal(v.text.strip())


def has_null(v):
    if isinstance(v, list):
        return any(has_null(x) for x in v)
    return v is None or (isinstance(v, dict) and any(has_null(x) for x in v.values()))


def selftest(program):
    dmn = program["dmn"]
    model = program_path(program["id"], dmn["model"]).read_text()
    adapter = load_module(program["id"], "oracle")
    defaults = getattr(adapter, "context_defaults", {})
    # An adapter whose TCK file lists values the engine computes more precisely says how close is equal.
    same = getattr(adapter, "tck_equal", lambda name, got, want: got == want)
    bad = ran = 0
    for tc in ET.parse(program_path(program["id"], dmn["tests"])).getroot().findall("{*}testCase"):
        inputs = {n.get("name"): xml_value(n) for n in tc.findall("{*}inputNode")}
        if has_null(inputs):
            print(f"case {tc.get('id')}: skipped (null input)")
            continue
        got = evaluate(model, {**defaults, **inputs})
        ran += 1
        for rn in tc.findall("{*}resultNode"):
            want = xml_value(rn.find("{*}expected"))
            ok = same(rn.get("name"), got.get(rn.get("name")), want)
            bad += not ok
            print(f"case {tc.get('id')}: {rn.get('name')} = {got.get(rn.get('name'))!r} expected {want!r} {'ok' if ok else 'MISMATCH'}")
    print(f"selftest: {ran} cases run, {bad} mismatches")
    return bad == 0


def dmn_row(program, case, model, adapter):
    context = {**getattr(adapter, "context_defaults", {}), **adapter.to_context(case)}
    got = evaluate(model, context)
    outputs = program["dmn"]["outputs"]
    missing = [k for k in outputs if got.get(k) is None]
    if missing:
        sys.exit(f"case {case['id']}: oracle returned no value for {missing}: {got}")
    values = {outputs[k]: got[k] for k in outputs}
    if hasattr(adapter, "from_dmn"):  # a nested DMN result reshaped into the descriptor's fields
        values = adapter.from_dmn(values)
    return {"id": case["id"], **values}


def run_all(program):
    cases = load_inputs(program["id"])
    adapter = load_module(program["id"], "oracle")
    out, diffs = [], 0
    model = program_path(program["id"], program["dmn"]["model"]).read_text() if program["oracle"] == "drools" else None
    for case in cases:
        if program["oracle"] == "drools":
            row = dmn_row(program, case, model, adapter)
            if hasattr(adapter, "check") and (diff := adapter.check(case, row)):
                diffs += 1
                print(f"case {case['id']} {case.get('tags')}: {diff}")
        else:
            got = adapter.decide(case)
            row = {"id": case["id"], **{f: got[f] for f in field_names(program)}}
        out.append(row)
    path = heldout_path(program["id"], "expected.jsonl")
    path.write_text("".join(dumps(r) + "\n" for r in out))
    print(f"wrote {len(out)} rows to {path}; reference differs on {diffs}")


def start(program):
    if program["oracle"] != "drools":
        sys.exit(f"{program['id']} has no Drools oracle")
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--program", required=True, help="folder name under benchmark/p1, e.g. 0087")
    ap.add_argument("--start", action="store_true")
    ap.add_argument("--stop", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    program = load_program(args.program)
    if args.start:
        start(program)
    elif args.stop:
        subprocess.run(["docker", "stop", NAME], check=True)
    elif args.selftest:
        if program["oracle"] != "drools":
            sys.exit(f"--selftest is DMN-only; {program['id']} uses its Python oracle")
        sys.exit(0 if selftest(program) else 1)
    else:
        run_all(program)


if __name__ == "__main__":
    main()
