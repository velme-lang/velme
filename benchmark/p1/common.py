"""Shared helpers: program descriptors, held-out files, exact-decimal JSON, trial output, prices."""
import importlib.util
import json
import sys
from decimal import Decimal
from pathlib import Path

HERE = Path(__file__).parent

# $ per million tokens (input, output). P1e confirms these against the provider's price list.
PRICES = {
    "claude-haiku-5-5": (1.0, 5.0),
    "claude-sonnet-5-5": (3.0, 15.0),
}
CACHE_WRITE_MULT = 1.25
CACHE_READ_MULT = 0.1


def loads(text):
    """json.loads with every number an exact Decimal."""
    return json.loads(text, parse_float=Decimal, parse_int=Decimal)


def dumps(v):
    """JSON with Decimal written as its exact plain text (float is written as is, so the scorer can see it)."""
    if isinstance(v, Decimal):
        return format(v, "f")
    if isinstance(v, dict):
        return "{" + ", ".join(f"{json.dumps(k)}: {dumps(x)}" for k, x in v.items()) + "}"
    if isinstance(v, list):
        return "[" + ", ".join(dumps(x) for x in v) + "]"
    return json.dumps(v)


def program_dir(program):
    return HERE / program


def load_program(program):
    """The program descriptor: goal, source, entry, inputs, oracle and per-field comparison."""
    return loads((program_dir(program) / "program.json").read_text())


def field_names(program):
    return [f["name"] for f in program["fields"]]


def program_path(program, relative):
    """A path in the descriptor, relative to a program folder (or to benchmark/p1 when it starts with `.cache/`)."""
    p = Path(relative)
    return p if p.is_absolute() else (HERE / p if relative.startswith(".cache/") else program_dir(program) / p)


def load_module(program, name):
    """Load `<program>/<name>.py`; its own folder is on sys.path while it runs, so it can import its neighbours."""
    path = program_dir(program) / f"{name}.py"
    folder = str(program_dir(program))
    sys.path.insert(0, folder)
    try:
        spec = importlib.util.spec_from_file_location(f"p1_{program}_{name}", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    finally:
        sys.path.remove(folder)


def read_jsonl(path):
    return [loads(line) for line in Path(path).read_text().splitlines() if line.strip()]


def heldout_path(program, name):
    return program_dir(program) / "heldout" / name


def load_inputs(program):
    return read_jsonl(heldout_path(program, "inputs.jsonl"))


def load_expected(program):
    return {row["id"]: row for row in read_jsonl(heldout_path(program, "expected.jsonl"))}


def load_kinds(program):
    """id -> `boundary` or `random`, validated. Only 0087 may derive a kind from its old tags; others need kinds.jsonl."""
    sidecar = heldout_path(program, "kinds.jsonl")
    if sidecar.exists():
        kinds = {row["id"]: row["kind"] for row in read_jsonl(sidecar)}
        bad = {id_: kind for id_, kind in kinds.items() if kind not in ("boundary", "random")}
        if bad:
            raise ValueError(f"{sidecar}: kinds must be boundary or random, got {bad}")
        return kinds
    if program != "0087":
        raise ValueError(f"{program}/heldout/kinds.jsonl is missing; every program but 0087 must carry one")
    return {case["id"]: ("random" if case.get("tags") == ["random"] else "boundary") for case in load_inputs(program)}


def trial_dir(root, program, arm, model, trial):
    """`<root>/<program>/<arm>/<model>/trial-N`: the one run layout every runner writes."""
    return Path(root) / program / arm / model / f"trial-{trial}"


def write_trial(trial_path, rows, meta):
    """Write into the final `trial-N` directory; rows: [{"id", "output": dict | None, "error": str | None}]."""
    d = Path(trial_path)
    d.mkdir(parents=True, exist_ok=True)
    (d / "results.jsonl").write_text("".join(dumps(r) + "\n" for r in rows))
    (d / "meta.json").write_text(json.dumps(meta, indent=1) + "\n")


def prices(model):
    try:
        return PRICES[model]
    except KeyError:
        sys.exit(f"no price for model {model!r}; add it to common.PRICES")


def cost(meta, model):
    """$ as billed: uncached input plus cache reads at 0.1x and writes at 1.25x."""
    pin, pout = prices(model)
    return (meta["tokens_in"] * pin + meta["tokens_out"] * pout + meta.get("cache_read", 0) * CACHE_READ_MULT * pin
            + meta.get("cache_write", 0) * CACHE_WRITE_MULT * pin) / 1e6


def cost_uncached(meta, model):
    """$ if every input token, cached or not, were billed at the plain input price."""
    pin, pout = prices(model)
    return ((meta["tokens_in"] + meta.get("cache_read", 0) + meta.get("cache_write", 0)) * pin
            + meta["tokens_out"] * pout) / 1e6
