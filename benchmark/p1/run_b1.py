"""B1 arm: one-shot Python. One request, one function, no tests and no feedback.

  python run_b1.py --program 0087 --model ID --trials N --out runs [--fake-reply DIR] [--no-prompt-cache]

Its prompt is run_b2.build_system(program, include_tests=False): the same prefix as B2's first turn with the tests
instruction replaced by a single-file reply. Same model, max tokens and trial layout as B2; output goes to
<out>/<program>/b1/<model>/trial-K/. `build_ok` means a solution.py was extracted and imports.
"""
import argparse
import time
from pathlib import Path

from common import load_program, prices, trial_dir, write_trial
from run_b2 import build_system, call_api, extract_solution, first_message, imports_ok, run_heldout

ARM = "b1"


def trial(args, program, k, system):
    t0 = time.monotonic()
    messages = [first_message(False)]
    use = {"tokens_in": 0, "tokens_out": 0, "cache_read": 0, "cache_write": 0, "calls": 0}
    if args.fake_reply:
        f = Path(args.fake_reply) / "reply-1.txt"
        if not f.exists():
            raise SystemExit(f"fake reply {f} is missing")
        text = f.read_text()
    else:
        resp = call_api(args.model, system, messages, not args.no_prompt_cache)
        text = "".join(b.get("text", "") for b in resp["content"] if b["type"] == "text")
        u = resp["usage"]
        use["tokens_in"] += u.get("input_tokens", 0)
        use["tokens_out"] += u.get("output_tokens", 0)
        use["cache_read"] += u.get("cache_read_input_tokens") or 0
        use["cache_write"] += u.get("cache_creation_input_tokens") or 0
    use["calls"] = 1
    solution = extract_solution(text)
    # Wall is the model turn only, like B2's loop and Velme's build; held-out execution is not arm time.
    wall = round(time.monotonic() - t0, 2)
    build_ok = solution is not None and imports_ok(solution)
    rows = run_heldout(program, solution)
    meta = {"program": program["id"], "arm": ARM, "model": args.model, "trial": k, "build_ok": build_ok, "wall_s": wall, **use}
    return rows, meta, solution, text


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--program", required=True, help="folder name under benchmark/p1, e.g. 0087")
    ap.add_argument("--model", required=True)
    ap.add_argument("--trials", type=int, default=1)
    ap.add_argument("--out", required=True, help="run root; trials go to <out>/<program>/b1/<model>/trial-N")
    ap.add_argument("--fake-reply", help="directory with reply-1.txt instead of calling the API")
    ap.add_argument("--no-prompt-cache", action="store_true")
    args = ap.parse_args()
    if not args.fake_reply:
        prices(args.model)  # fail before paying for a run whose cost cannot be reported
    program = load_program(args.program)
    system = build_system(program, False)
    for k in range(1, args.trials + 1):
        rows, meta, solution, text = trial(args, program, k, system)
        d = trial_dir(args.out, program["id"], ARM, args.model, k)
        write_trial(d, rows, meta)
        (d / "solution.py").write_text(solution or "")
        (d / "reply.txt").write_text(text)
        ok = sum(r["error"] is None for r in rows)
        print(f"trial {k}: build_ok={meta['build_ok']} calls={meta['calls']} tokens={meta['tokens_in']}/{meta['tokens_out']} "
              f"cache={meta['cache_read']}/{meta['cache_write']} wall={meta['wall_s']}s ran_ok={ok}/{len(rows)}")


if __name__ == "__main__":
    main()
