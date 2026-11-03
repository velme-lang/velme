"""Generate one program's held-out inputs: seeded, boundary cases from its own gen.py, plus random ones.

Usage: python gen_inputs.py --program 0087 [--seed 1] [--n 200] [--out <program>/heldout/inputs.jsonl]

The program's `gen.py` returns `{"id", <input keys...>, "kind"}` cases (`kind` is `boundary` or `random`). `kind` is
written to the `kinds.jsonl` sidecar, never into inputs.jsonl, so regenerating 0087 with seed 1 keeps its committed
inputs byte-identical.
"""
import argparse
import sys
from pathlib import Path

from common import dumps, load_module, load_program, program_dir


def kind_of(case, program):
    """Validate the case kind; only 0087 may still derive it from its old tags."""
    kind = case.get("kind")
    if kind is None and program == "0087":
        kind = "random" if case.get("tags") == ["random"] else "boundary"
    if kind not in ("boundary", "random"):
        raise ValueError(f"{program} case {case.get('id')}: kind must be boundary or random, got {kind!r}")
    return kind


def generate(program, seed, n):
    cases = load_module(program, "gen").generate(seed, n)
    if len(cases) != n:
        sys.exit(f"{program}/gen.py returned {len(cases)} cases, expected {n}")
    for case in cases:
        kind_of(case, program)  # fail before writing anything
    return cases


def write(program, seed, n, out):
    cases = generate(program, seed, n)
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("".join(dumps({k: v for k, v in c.items() if k != "kind"}) + "\n" for c in cases))
    kinds = [{"id": c["id"], "kind": kind_of(c, program)} for c in cases]
    (out.parent / "kinds.jsonl").write_text("".join(dumps(k) + "\n" for k in kinds))
    counts = {"boundary": 0, "random": 0}
    for k in kinds:
        counts[k["kind"]] += 1
    print(f"wrote {len(cases)} cases to {out} and {out.parent / 'kinds.jsonl'}; kinds: {counts}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--program", required=True, help="folder name under benchmark/p1, e.g. 0087")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--n", type=int, default=200)
    ap.add_argument("--out", help="default: <program>/heldout/inputs.jsonl")
    args = ap.parse_args()
    load_program(args.program)  # fail early when the folder has no descriptor
    out = args.out or str(program_dir(args.program) / "heldout" / "inputs.jsonl")
    write(args.program, args.seed, args.n, out)


if __name__ == "__main__":
    main()
