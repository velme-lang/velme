"""Score runs against the oracle's expected.jsonl.

  python score.py runs/a runs/b ... [--price-in 3] [--price-out 15]

A case passes when all 10 fields match: Text and Boolean exactly, numbers as exact Decimal, except
required_monthly_installment, which is compared after quantizing both sides to 8 decimal places (ROUND_HALF_EVEN):
the DMN engine evaluates with 34 significant digits and Velme with 28, so the long irrational tails differ.
A float anywhere in a B2 output fails the case and is counted in its own column. Prints one markdown table per
run directory and writes them all to <parent of first run>/report.md.
"""
import argparse
import json
from decimal import ROUND_HALF_EVEN, Decimal
from pathlib import Path

from common import FIELDS, HELDOUT, loads

Q8 = Decimal("1e-8")


def is_num(v):
    return isinstance(v, (Decimal, int)) and not isinstance(v, bool)


def field_ok(field, got, want):
    if isinstance(want, bool):
        return isinstance(got, bool) and got == want
    if isinstance(want, str):
        return isinstance(got, str) and got == want
    if not is_num(got):
        return False
    got = Decimal(got)
    if field == "required_monthly_installment":
        return got.quantize(Q8, ROUND_HALF_EVEN) == want.quantize(Q8, ROUND_HALF_EVEN)
    return got == want


def score_case(row, want):
    """(failed fields, has float). A row with no output fails every field and is reported as 'no output' by the caller."""
    out = row.get("output")
    bad = [f for f in FIELDS if not isinstance(out, dict) or f not in out or not field_ok(f, out[f], want[f])]
    return bad, bool(row.get("float"))


def read_jsonl(path):
    return [loads(l) for l in Path(path).read_text().splitlines() if l.strip()]


def score_trial(trial_dir, expected, boundary_ids):
    rows = {r["id"]: r for r in read_jsonl(trial_dir / "results.jsonl")}
    res = {"passed": 0, "total": len(expected), "b_pass": 0, "b_total": 0, "r_pass": 0, "r_total": 0,
           "float": 0, "no_output": 0, "fields": {f: 0 for f in FIELDS}}
    for id_, want in expected.items():
        row = rows.get(id_, {"output": None})
        bad, has_float = score_case(row, want)
        ok = not bad and not has_float
        kind = "b" if id_ in boundary_ids else "r"
        res["passed"] += ok
        res[kind + "_pass"] += ok
        res[kind + "_total"] += 1
        res["float"] += has_float
        if not isinstance(row.get("output"), dict):
            res["no_output"] += 1
        else:
            for f in bad:
                res["fields"][f] += 1
    return res


def cost(meta, pin, pout):
    m = meta
    return (m["tokens_in"] * pin + m["tokens_out"] * pout + m.get("cache_read", 0) * 0.1 * pin + m.get("cache_write", 0) * 1.25 * pin) / 1e6


def mean(xs):
    return sum(xs) / len(xs) if xs else 0.0


def report(run_dir, expected, boundary_ids, pin, pout):
    trials = sorted(Path(run_dir).glob("trial-*"))
    scores = [score_trial(t, expected, boundary_ids) for t in trials]
    metas = [json.loads((t / "meta.json").read_text()) for t in trials]
    rate = [s["passed"] / s["total"] for s in scores]
    pct = lambda x: f"{100 * x:.1f}%"  # noqa: E731
    sum_r = lambda k: sum(s[k] for s in scores)  # noqa: E731
    rows = [
        ("trials", len(trials)),
        ("held-out pass rate mean / min / max", f"{pct(mean(rate))} / {pct(min(rate, default=0))} / {pct(max(rate, default=0))}"),
        ("boundary-tagged pass rate", pct(sum_r("b_pass") / max(1, sum_r("b_total")))),
        ("random-tagged pass rate", pct(sum_r("r_pass") / max(1, sum_r("r_total")))),
        ("trials whose build / tests never passed", sum(not m["build_ok"] for m in metas)),
        ("cases with a float (failed)", sum_r("float")),
        ("cases with no output (error)", sum_r("no_output")),
    ]
    rows += [(f"field failures: {f}", sum(s["fields"][f] for s in scores)) for f in FIELDS]
    rows += [
        ("mean model calls", f"{mean([m['calls'] for m in metas]):.2f}"),
        ("mean tokens in", f"{mean([m['tokens_in'] for m in metas]):.0f}"),
        ("mean tokens out", f"{mean([m['tokens_out'] for m in metas]):.0f}"),
        ("mean cost $", f"{mean([cost(m, pin, pout) for m in metas]):.4f}"),
        ("mean wall time s", f"{mean([m['wall_s'] for m in metas]):.1f}"),
    ]
    head = f"### {run_dir} ({metas[0]['arm']}, {metas[0]['model']})" if metas else f"### {run_dir} (no trials)"
    return head + "\n\n| metric | value |\n|---|---|\n" + "\n".join(f"| {k} | {v} |" for k, v in rows) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="+")
    ap.add_argument("--price-in", type=float, default=3.0, help="$ per million input tokens")
    ap.add_argument("--price-out", type=float, default=15.0, help="$ per million output tokens")
    args = ap.parse_args()
    expected = {r["id"]: r for r in read_jsonl(HELDOUT / "expected.jsonl")}
    boundary_ids = {c["id"] for c in read_jsonl(HELDOUT / "inputs.jsonl") if c["tags"] != ["random"]}
    text = "\n".join(report(r, expected, boundary_ids, args.price_in, args.price_out) for r in args.runs)
    print(text)
    (Path(args.runs[0]).parent / "report.md").write_text(text)


if __name__ == "__main__":
    main()
