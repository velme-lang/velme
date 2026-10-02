"""Run solution.decide_loan on every held-out input; used by run_b2.py in a subprocess so a crash or a hang
cannot take the harness down.  python b2_exec.py SOLUTION_DIR INPUTS.jsonl OUT.jsonl [PER_CASE_SECONDS]

Writes one line per case, flushed as it goes: {"id", "output", "error", "float"}.  "float" is true when any float
appears in the output; those floats are written as their repr text so the line stays valid JSON.
"""
import copy
import signal
import sys
from decimal import Decimal

from common import dumps, loads


def strip_floats(v):
    """(value with floats replaced by Decimal(repr), whether there was a float)."""
    if isinstance(v, float):
        return Decimal(repr(v)), True
    if isinstance(v, dict):
        items = {k: strip_floats(x) for k, x in v.items()}
        return {k: x for k, (x, _) in items.items()}, any(f for _, f in items.values())
    if isinstance(v, (list, tuple)):
        items = [strip_floats(x) for x in v]
        return [x for x, _ in items], any(f for _, f in items)
    return v, False


def alarm(signum, frame):
    raise TimeoutError("case timed out")


def main():
    sol_dir, inputs, out_path = sys.argv[1:4]
    limit = float(sys.argv[4]) if len(sys.argv) > 4 else 10.0
    sys.path.append(sol_dir)
    cases = [loads(l) for l in open(inputs).read().splitlines() if l.strip()]
    if hasattr(signal, "setitimer"):
        signal.signal(signal.SIGALRM, alarm)
    with open(out_path, "w") as out:
        try:
            import solution
            err0 = None
        except BaseException as e:  # noqa: BLE001 - any import failure fails every case
            err0 = f"import failed: {type(e).__name__}: {e}"
        for c in cases:
            row = {"id": c["id"], "output": None, "error": err0, "float": False}
            if err0 is None:
                try:
                    if hasattr(signal, "setitimer"):
                        signal.setitimer(signal.ITIMER_REAL, limit)
                    res = solution.decide_loan(*copy.deepcopy([c["applicant"], c["product"], c["bureau"]]))
                    row["output"], row["float"] = strip_floats(res)
                    dumps(row)
                except BaseException as e:  # noqa: BLE001 - a crash is a failed case, not a harness error
                    row.update(output=None, error=f"{type(e).__name__}: {e}"[:200])
                finally:
                    if hasattr(signal, "setitimer"):
                        signal.setitimer(signal.ITIMER_REAL, 0)
            try:
                line = dumps(row)
            except TypeError as e:
                line = dumps({**row, "output": None, "error": f"unserializable output: {e}"})
            out.write(line + "\n")
            out.flush()


main()
