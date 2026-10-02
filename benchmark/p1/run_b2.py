"""B2 arm: Python + tests loop through the Anthropic Messages API.

  python run_b2.py --model ID --trials N --out runs/NAME [--fake-reply DIR]

The model gets requirements.md, signatures.py and examples.json verbatim and returns solution.py and
test_solution.py; pytest runs; failures go back (last 80 lines) for at most 3 fix rounds, so at most 4 model calls.
The final solution.py is then run on every held-out input in a subprocess. ANTHROPIC_API_KEY is read from the
environment and never printed. --fake-reply DIR reads canned replies reply-1.txt, reply-2.txt, ... instead of calling the API.
Output: <out>/trial-K/{results.jsonl,meta.json} (+ solution.py, test_solution.py, last_pytest.txt).
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

from common import CASE, HELDOUT, HERE, load_inputs, loads, write_trial

API = "https://api.anthropic.com/v1/messages"
MAX_TOKENS = 8192  # Velme's default max_output_tokens for anthropic
FIX_ROUNDS = 3

SYSTEM = """You are writing production Python (3.13, standard library only, plus pytest for tests).

Implement the business rules in <requirements> exactly, as the functions declared in <signatures> (same names, \
parameters and order, in one module). <examples> lists known input/output pairs as JSON: "function", "args", "expected". \
JSON numbers are exact decimal text: read them with Decimal (for instance json.loads(text, parse_float=Decimal, \
parse_int=Decimal)), never through float. Number inputs reach your functions as decimal.Decimal; return numbers as \
Decimal or int, exact, never float, and round nothing.

Reply with exactly two fenced code blocks and no other code: first the full contents of solution.py, then the full \
contents of test_solution.py. test_solution.py is run with `python -m pytest -q` in the same directory as solution.py \
and must contain a test for every example. Short notes outside the code blocks are fine.

<requirements>
{requirements}
</requirements>

<signatures>
{signatures}
</signatures>

<examples>
{examples}
</examples>"""

FEEDBACK = """The tests failed (`python -m pytest -q`, exit code {code}). Last lines of the output:

<pytest_output>
{tail}
</pytest_output>

Fix the problem, whether it is in solution.py or in the tests, and return both files again in full, in the same \
format: exactly two fenced code blocks, solution.py then test_solution.py."""


def call_api(model, system, messages):
    body = json.dumps({"model": model, "max_tokens": MAX_TOKENS, "system": system, "messages": messages}).encode()
    headers = {"x-api-key": os.environ["ANTHROPIC_API_KEY"], "anthropic-version": "2023-06-01", "content-type": "application/json"}
    for attempt in range(6):  # transport-level retries for overload / rate limits; not counted as calls
        try:
            with urllib.request.urlopen(urllib.request.Request(API, body, headers), timeout=300) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            if e.code not in (429, 500, 502, 503, 529) or attempt == 5:
                sys.exit(f"API error {e.code}: {e.read().decode()[:500]}")
        except (urllib.error.URLError, TimeoutError) as e:
            if attempt == 5:
                sys.exit(f"API unreachable: {e}")
        time.sleep(2 ** attempt)


def extract_files(text):
    blocks = re.findall(r"```[\w.+-]*\n(.*?)```", text, re.S)
    return (blocks[0], blocks[1]) if len(blocks) >= 2 else None


def run_pytest(solution, tests):
    with tempfile.TemporaryDirectory(prefix="b2-pytest-") as d:
        (Path(d) / "solution.py").write_text(solution)
        (Path(d) / "test_solution.py").write_text(tests)
        try:
            r = subprocess.run([sys.executable, "-m", "pytest", "-q"], cwd=d, capture_output=True, text=True, timeout=60,
                               env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
            return r.returncode, r.stdout + r.stderr
        except subprocess.TimeoutExpired:
            return -1, "pytest timed out after 60 s"


def model_loop(args, system):
    """Up to 1 + FIX_ROUNDS calls. Returns (solution, tests, tests_passed, usage totals, last pytest output)."""
    messages = [{"role": "user", "content": "Write solution.py and test_solution.py."}]
    use = {"tokens_in": 0, "tokens_out": 0, "cache_read": 0, "cache_write": 0, "calls": 0}
    solution = tests = None
    passed, output = False, ""
    for rnd in range(FIX_ROUNDS + 1):
        if args.fake_reply:
            f = Path(args.fake_reply) / f"reply-{rnd + 1}.txt"
            if not f.exists():
                sys.exit(f"fake reply {f} is missing")
            text, u = f.read_text(), {}
        else:
            resp = call_api(args.model, system, messages)
            text = "".join(b.get("text", "") for b in resp["content"] if b["type"] == "text")
            u = resp["usage"]
        use["calls"] += 1
        use["tokens_in"] += u.get("input_tokens", 0)
        use["tokens_out"] += u.get("output_tokens", 0)
        use["cache_read"] += u.get("cache_read_input_tokens") or 0
        use["cache_write"] += u.get("cache_creation_input_tokens") or 0
        messages.append({"role": "assistant", "content": text})
        files = extract_files(text)
        if files:
            solution, tests = files
            code, output = run_pytest(solution, tests)
            passed = code == 0
            if passed:
                break
            tail = "\n".join(output.splitlines()[-80:])
        else:
            code, tail, passed = -2, "", False
            output = "reply did not contain two fenced code blocks"
            tail = output
        if rnd < FIX_ROUNDS:
            messages.append({"role": "user", "content": FEEDBACK.format(code=code, tail=tail)})
    return solution, tests, passed, use, output


def run_heldout(solution, timeout=300):
    cases = load_inputs()
    if solution is None:
        return [{"id": c["id"], "output": None, "error": "no solution.py produced"} for c in cases]
    with tempfile.TemporaryDirectory(prefix="b2-exec-") as d:
        (Path(d) / "solution.py").write_text(solution)
        out = Path(d) / "out.jsonl"
        try:
            subprocess.run([sys.executable, str(HERE / "b2_exec.py"), d, str(HELDOUT / "inputs.jsonl"), str(out)],
                           capture_output=True, text=True, timeout=timeout, cwd=d)
        except subprocess.TimeoutExpired:
            pass  # the cases written so far stay; the rest are marked below
        done = {}
        for line in (out.read_text().splitlines() if out.exists() else []):
            try:
                r = loads(line)
            except ValueError:
                continue  # a torn last line from the kill
            done[r["id"]] = r
    return [done.get(c["id"], {"id": c["id"], "output": None, "error": "not run (overall timeout)"}) for c in cases]


def trial(args, k, system):
    t0 = time.monotonic()
    solution, tests, passed, use, last = model_loop(args, system)
    wall = round(time.monotonic() - t0, 2)
    rows = run_heldout(solution)
    meta = {"arm": "b2", "model": args.model, "trial": k, "build_ok": passed, "wall_s": wall, **use,
            "fix_rounds": use["calls"] - 1}
    return rows, meta, solution, tests, last


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--trials", type=int, default=1)
    ap.add_argument("--out", required=True)
    ap.add_argument("--fake-reply", help="directory with reply-1.txt, reply-2.txt, ... instead of calling the API")
    args = ap.parse_args()
    if not args.fake_reply and not os.environ.get("ANTHROPIC_API_KEY"):
        sys.exit("ANTHROPIC_API_KEY is not set")
    system = SYSTEM.format(requirements=(CASE / "requirements.md").read_text(), signatures=(CASE / "signatures.py").read_text(),
                           examples=(CASE / "examples.json").read_text())
    for k in range(1, args.trials + 1):
        rows, meta, solution, tests, last = trial(args, k, system)
        write_trial(args.out, k, rows, meta)
        d = Path(args.out) / f"trial-{k}"
        (d / "solution.py").write_text(solution or "")
        (d / "test_solution.py").write_text(tests or "")
        (d / "last_pytest.txt").write_text(last)
        ok = sum(r["error"] is None for r in rows)
        print(f"trial {k}: tests_passed={meta['build_ok']} calls={meta['calls']} tokens={meta['tokens_in']}/{meta['tokens_out']} "
              f"wall={meta['wall_s']}s ran_ok={ok}/{len(rows)}")


if __name__ == "__main__":
    main()
