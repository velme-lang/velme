"""B2 arm: Python + tests loop through the Anthropic Messages API.

  python run_b2.py --program 0087 --model ID --trials N --out runs [--fake-reply DIR] [--no-prompt-cache]

The model gets the program's requirements.md, signatures.py and examples.json verbatim and returns solution.py and
test_solution.py; pytest runs; failures go back (last 80 lines) for at most 3 fix rounds, so at most 4 model calls.
The final solution.py is then run on every held-out input in a subprocess. ANTHROPIC_API_KEY is read from the
environment and never printed. --fake-reply DIR reads canned replies reply-1.txt, reply-2.txt, ... instead of calling
the API. Output: <out>/<program>/b2/<model>/trial-K/{results.jsonl,meta.json} (+ solution.py, test_solution.py,
last_pytest.txt).

The prompt is built once, in build_system(program, include_tests); run_b1.py calls the same function with False.
Prompt caching is on unless --no-prompt-cache; the cache_control breakpoint sits on the stable system prefix.
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

from common import HERE, heldout_path, load_inputs, load_program, loads, prices, program_path, trial_dir, write_trial

API = "https://api.anthropic.com/v1/messages"
MAX_TOKENS = 8192  # Velme's default max_output_tokens for anthropic
FIX_ROUNDS = 3

BASE = """You are writing production Python (3.13, standard library only).

Implement the business rules in <requirements> exactly, as the functions declared in <signatures> (same names, \
parameters and order, in one module). <examples> lists known input/output pairs as JSON: "function", "args", "expected". \
JSON numbers are exact decimal text: read them with Decimal (for instance json.loads(text, parse_float=Decimal, \
parse_int=Decimal)), never through float. Number inputs reach your functions as decimal.Decimal; return numbers as \
Decimal or int, exact, never float, and round nothing.

<requirements>
{requirements}
</requirements>

<signatures>
{signatures}
</signatures>

<examples>
{examples}
</examples>"""

TESTS_INSTRUCTION = """

Also write test_solution.py, with one pytest test for every example. Reply with exactly two fenced code blocks and no \
other code: first the full contents of solution.py, then the full contents of test_solution.py. test_solution.py runs \
with `python -m pytest -q` in the same directory as solution.py. Short notes outside the code blocks are fine."""

SOLUTION_INSTRUCTION = """

Reply with exactly one fenced code block and no other code: the full contents of solution.py. Short notes outside the \
code block are fine."""

FEEDBACK = """The tests failed (`python -m pytest -q`, exit code {code}). Last lines of the output:

<pytest_output>
{tail}
</pytest_output>

Fix the problem, whether it is in solution.py or in the tests, and return both files again in full, in the same \
format: exactly two fenced code blocks, solution.py then test_solution.py."""


def build_system(program, include_tests):
    """The single prompt builder: B2 adds the tests instruction, B1 keeps the same prefix without it."""
    system = BASE.format(requirements=program_path(program["id"], "requirements.md").read_text(),
                         signatures=program_path(program["id"], "signatures.py").read_text(),
                         examples=program_path(program["id"], "examples.json").read_text())
    return system + (TESTS_INSTRUCTION if include_tests else SOLUTION_INSTRUCTION)


def first_message(include_tests):
    return {"role": "user", "content": "Write solution.py and test_solution.py." if include_tests else "Write solution.py."}


def request_payload(model, system, messages, cache):
    """The Messages API body. The cache breakpoint is on the stable system prefix (requirements + signatures + examples)."""
    payload = {"model": model, "max_tokens": MAX_TOKENS, "messages": messages}
    if cache:
        payload["system"] = [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}]
    else:
        payload["system"] = system
    return payload


def call_api(model, system, messages, cache):
    body = json.dumps(request_payload(model, system, messages, cache)).encode()
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


def extract_solution(text):
    blocks = re.findall(r"```[\w.+-]*\n(.*?)```", text, re.S)
    return blocks[0] if blocks else None


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


def imports_ok(solution):
    with tempfile.TemporaryDirectory(prefix="b1-import-") as d:
        (Path(d) / "solution.py").write_text(solution)
        try:
            r = subprocess.run([sys.executable, "-c", "import solution"], cwd=d, capture_output=True, text=True, timeout=60,
                               env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
            return r.returncode == 0
        except subprocess.TimeoutExpired:
            return False


def model_loop(args, program, system):
    """Up to 1 + FIX_ROUNDS calls. Returns (solution, tests, tests_passed, usage totals, last pytest output)."""
    messages = [first_message(True)]
    use = {"tokens_in": 0, "tokens_out": 0, "cache_read": 0, "cache_write": 0, "calls": 0}
    cache = not args.no_prompt_cache
    solution = tests = None
    passed, output = False, ""
    for rnd in range(FIX_ROUNDS + 1):
        if args.fake_reply:
            f = Path(args.fake_reply) / f"reply-{rnd + 1}.txt"
            if not f.exists():
                sys.exit(f"fake reply {f} is missing")
            text, u = f.read_text(), {}
        else:
            resp = call_api(args.model, system, messages, cache)
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


def run_heldout(program, solution, timeout=300):
    cases = load_inputs(program["id"])
    if solution is None:
        return [{"id": c["id"], "output": None, "error": "no solution.py produced"} for c in cases]
    with tempfile.TemporaryDirectory(prefix="python-exec-") as d:
        (Path(d) / "solution.py").write_text(solution)
        out = Path(d) / "out.jsonl"
        cmd = [sys.executable, str(HERE / "b2_exec.py"), d, str(heldout_path(program["id"], "inputs.jsonl")), str(out),
               "--entry", program["entry"], "--limit", "10"]
        for key in program["inputs"]:
            cmd += ["--in-key", key]
        try:
            subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, cwd=d)
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


def trial(args, program, k, system):
    t0 = time.monotonic()
    solution, tests, passed, use, last = model_loop(args, program, system)
    wall = round(time.monotonic() - t0, 2)
    rows = run_heldout(program, solution)
    meta = {"program": program["id"], "arm": "b2", "model": args.model, "trial": k, "build_ok": passed, "wall_s": wall,
            **use, "fix_rounds": use["calls"] - 1}
    return rows, meta, solution, tests, last


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--program", required=True, help="folder name under benchmark/p1, e.g. 0087")
    ap.add_argument("--model", required=True)
    ap.add_argument("--trials", type=int, default=1)
    ap.add_argument("--out", required=True, help="run root; trials go to <out>/<program>/b2/<model>/trial-N")
    ap.add_argument("--fake-reply", help="directory with reply-1.txt, reply-2.txt, ... instead of calling the API")
    ap.add_argument("--no-prompt-cache", action="store_true")
    args = ap.parse_args()
    if not args.fake_reply:
        prices(args.model)  # fail before paying for a run whose cost cannot be reported
    if not args.fake_reply and not os.environ.get("ANTHROPIC_API_KEY"):
        sys.exit("ANTHROPIC_API_KEY is not set")
    program = load_program(args.program)
    system = build_system(program, True)
    for k in range(1, args.trials + 1):
        rows, meta, solution, tests, last = trial(args, program, k, system)
        d = trial_dir(args.out, program["id"], "b2", args.model, k)
        write_trial(d, rows, meta)
        (d / "solution.py").write_text(solution or "")
        (d / "test_solution.py").write_text(tests or "")
        (d / "last_pytest.txt").write_text(last)
        ok = sum(r["error"] is None for r in rows)
        print(f"trial {k}: tests_passed={meta['build_ok']} calls={meta['calls']} tokens={meta['tokens_in']}/{meta['tokens_out']} "
              f"cache={meta['cache_read']}/{meta['cache_write']} wall={meta['wall_s']}s ran_ok={ok}/{len(rows)}")


if __name__ == "__main__":
    main()
