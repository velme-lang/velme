"""Score runs against each program's expected.jsonl.

  python score.py runs [runs2 ...]

A run root holds `<program>/<arm>/<model>/trial-N/` (the layout every runner writes). Cases pass when every descriptor
field matches: Text and Boolean exactly, numbers as exact Decimal, except fields marked `decimals` (0087's installment
to 8 places, the DMN engine's 34 significant digits against Velme's 28), and fields marked `list`, compared in order:
a list of records field by field when the descriptor's `item` is a list of fields (0014's rankings), a list of values
each as `item` says when it is one field spec (invoice-validation's reasons). A float anywhere in an output fails the
case and gets its own column. A failed Velme build counts as 0 held-out cases for that trial, not as a missing trial.

Prints pooled metrics per model across all programs and trials, then per program, then the P1 go/no-go line per model;
writes the same text to <first root>/report.md.
"""
import argparse
import json
import sys
from decimal import ROUND_HALF_EVEN, Decimal, InvalidOperation
from pathlib import Path

from common import cost, cost_uncached, field_names, load_expected, load_kinds, load_program, program_dir, read_jsonl


def is_num(v):
    return isinstance(v, (Decimal, int)) and not isinstance(v, bool)


def field_ok(field, got, want):
    if field.get("compare") == "list":
        if not (isinstance(got, list) and len(got) == len(want)):
            return False
        item = field["item"]
        if isinstance(item, dict):  # a list of values, each compared as `item` says (invoice-validation's reasons)
            return all(field_ok(item, g, w) for g, w in zip(got, want))
        # a list of records, each compared field by field per `item`
        return all(isinstance(g, dict) and all(f["name"] in g and field_ok(f, g[f["name"]], w[f["name"]]) for f in item)
                   for g, w in zip(got, want))
    if isinstance(want, bool):
        return isinstance(got, bool) and got == want
    if isinstance(want, str):
        return isinstance(got, str) and got == want
    if not is_num(got):
        return False
    got, want = Decimal(got), Decimal(want)
    if field.get("compare") == "decimals":
        q = Decimal(1).scaleb(-field.get("decimals", 0))
        try:
            return got.quantize(q, ROUND_HALF_EVEN) == want.quantize(q, ROUND_HALF_EVEN)
        except InvalidOperation:  # too many digits for the context precision: this case fails, the run goes on
            return False
    return got == want


def score_case(row, want, fields):
    """(failed field names, has float). A row with no output fails every field."""
    out = row.get("output")
    bad = [f["name"] for f in fields
           if not isinstance(out, dict) or f["name"] not in out or not field_ok(f, out[f["name"]], want[f["name"]])]
    return bad, bool(row.get("float"))


def score_trial(program, trial_dir, expected, kinds, failed=False):
    """Counts for one trial. `failed` forces a build failure to 0/N even if the runner left partial rows."""
    path = Path(trial_dir) / "results.jsonl"
    rows = {r["id"]: r for r in read_jsonl(path)} if path.exists() else {}
    fields = program["fields"]
    res = {"passed": 0, "total": len(expected), "b_pass": 0, "b_total": 0, "r_pass": 0, "r_total": 0,
           "float": 0, "no_output": 0, "fields": {f["name"]: 0 for f in fields}}
    for id_, want in expected.items():
        row = rows.get(id_, {"output": None})
        if failed:
            bad, has_float = field_names(program), False
        else:
            bad, has_float = score_case(row, want, fields)
        ok = not bad and not has_float
        kind = kinds.get(id_)
        if kind not in ("boundary", "random"):
            raise ValueError(f"{program['id']} case {id_}: kind must be boundary or random, got {kind!r}")
        kind = "b" if kind == "boundary" else "r"
        res["passed"] += ok
        res[kind + "_pass"] += ok
        res[kind + "_total"] += 1
        res["float"] += has_float
        if not isinstance(row.get("output"), dict):
            res["no_output"] += 1
        else:
            for name in bad:
                res["fields"][name] += 1
    return res


def discover(root):
    """Every trial under `<root>/<program>/<arm>/<model>/trial-N`, with its meta and score."""
    records = []
    for prog_dir in sorted(Path(root).iterdir()):
        if not prog_dir.is_dir() or not (program_dir(prog_dir.name) / "program.json").exists():
            continue
        program = load_program(prog_dir.name)
        expected, kinds = load_expected(prog_dir.name), load_kinds(prog_dir.name)
        for arm_dir in sorted(p for p in prog_dir.iterdir() if p.is_dir()):
            for model_dir in sorted(p for p in arm_dir.iterdir() if p.is_dir()):
                for trial in sorted(model_dir.glob("trial-*")):
                    if not (trial / "meta.json").exists():  # interrupted before write_trial: coverage reports it missing
                        continue
                    meta = json.loads((trial / "meta.json").read_text())
                    failed = arm_dir.name == "velme" and not meta.get("build_ok", True)
                    records.append({"program": prog_dir.name, "arm": arm_dir.name, "model": model_dir.name,
                                    "trial": trial.name, "meta": meta,
                                    "score": score_trial(program, trial, expected, kinds, failed)})
    return records


def metrics(records):
    n = len(records)
    mean = lambda xs: sum(xs) / len(xs) if xs else 0.0  # noqa: E731
    sum_s = lambda k: sum(r["score"][k] for r in records)  # noqa: E731
    return {
        "trials": n,
        "passed": sum_s("passed"), "total": sum_s("total"),
        "b_pass": sum_s("b_pass"), "b_total": sum_s("b_total"),
        "r_pass": sum_s("r_pass"), "r_total": sum_s("r_total"),
        "float": sum_s("float"), "no_output": sum_s("no_output"),
        "builds_failed": sum(not r["meta"].get("build_ok", True) for r in records),
        "cost": mean([cost(r["meta"], r["meta"].get("model", r["model"])) for r in records]),
        "cost_uncached": mean([cost_uncached(r["meta"], r["meta"].get("model", r["model"])) for r in records]),
        "tokens_in": mean([r["meta"].get("tokens_in", 0) for r in records]),
        "tokens_out": mean([r["meta"].get("tokens_out", 0) for r in records]),
        "calls": mean([r["meta"].get("calls", 0) for r in records]),
        "wall": mean([r["meta"].get("wall_s", 0) for r in records]),
    }


def group(records, key):
    out = {}
    for r in records:
        out.setdefault(key(r), []).append(r)
    return {k: metrics(v) for k, v in out.items()}


def pct(a, b):
    return f"{100 * a / b:.1f}%" if b else "-"


def table(title, rows):
    head = ("| group | trials | pass | boundary | random | floats | no output | failed builds | cost $ | uncached $ | "
            "tokens in/out | calls | wall s |")
    lines = [f"### {title}", "", head, "|" + "---|" * 13]
    for name, m in rows:
        lines.append(f"| {name} | {m['trials']} | {pct(m['passed'], m['total'])} | {pct(m['b_pass'], m['b_total'])} | "
                     f"{pct(m['r_pass'], m['r_total'])} | {m['float']} | {m['no_output']} | {m['builds_failed']} | "
                     f"{m['cost']:.4f} | {m['cost_uncached']:.4f} | {m['tokens_in']:.0f}/{m['tokens_out']:.0f} | "
                     f"{m['calls']:.2f} | {m['wall']:.1f} |")
    return "\n".join(lines)


def go_lines(pooled, coverage):
    """Per model, compare Velme and B2 only when both cover the same (program, trial) set; else INCOMPLETE."""
    lines = []
    for model in sorted({m for m, _ in pooled} | {m for m, _ in coverage}):
        v, b = pooled.get((model, "velme")), pooled.get((model, "b2"))
        vset, bset = coverage.get((model, "velme"), set()), coverage.get((model, "b2"), set())
        missing_v, missing_b = sorted(bset - vset), sorted(vset - bset)
        if not v or not b or missing_v or missing_b:
            details = []
            if not v:
                details.append("no Velme trials")
            if not b:
                details.append("no B2 trials")
            if missing_v:
                details.append("Velme missing " + ", ".join(f"{p}/{t}" for p, t in missing_v))
            if missing_b:
                details.append("B2 missing " + ", ".join(f"{p}/{t}" for p, t in missing_b))
            lines.append(f"- **{model}: INCOMPLETE** ({'; '.join(details)})")
            continue
        vp, bp = v["passed"] / v["total"] if v["total"] else 0, b["passed"] / b["total"] if b["total"] else 0
        vfail, bfail = v["b_total"] - v["b_pass"], b["b_total"] - b["b_pass"]
        cheaper = v["cost"] <= 1.5 * b["cost"]
        fewer = vfail < bfail
        if (vp >= bp and cheaper) or (vp == bp and fewer):
            verdict = "GO"
        else:
            verdict = "NO-GO"
        lines.append(f"- **{model}: {verdict}** (Velme {pct(v['passed'], v['total'])} at ${v['cost']:.4f} vs B2 "
                     f"{pct(b['passed'], b['total'])} at ${b['cost']:.4f}; boundary failures {vfail} vs {bfail})")
    return lines


def report(roots):
    records, seen = [], {}
    for root in roots:
        for r in discover(root):
            key = (r["program"], r["arm"], r["model"], r["trial"])
            if key in seen:
                sys.exit(f"duplicate trial {'/'.join(key)} in {seen[key]} and {root}")
            seen[key] = root
            records.append(r)
    pooled = group(records, lambda r: (r["model"], r["arm"]))
    per_program = group(records, lambda r: (r["program"], r["model"], r["arm"]))
    coverage = {}
    for r in records:
        coverage.setdefault((r["model"], r["arm"]), set()).add((r["program"], r["trial"]))
    parts = ["# P1 score", "", table("pooled by model", [(f"{m} / {a}", v) for (m, a), v in sorted(pooled.items())]), "",
             table("per program", [(f"{p} / {m} / {a}", v) for (p, m, a), v in sorted(per_program.items())]), "",
             "### go / no-go per model", ""] + go_lines(pooled, coverage)
    return "\n".join(parts) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="+", help="run root(s) holding <program>/<arm>/<model>/trial-N")
    args = ap.parse_args()
    text = report(args.runs)
    print(text)
    (Path(args.runs[0]) / "report.md").write_text(text)


if __name__ == "__main__":
    main()
