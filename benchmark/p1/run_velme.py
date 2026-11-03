"""Velme arm: `velme build` one program's plan, then run every held-out input through `velme run`.

  python run_velme.py --program 0087 --model ID --trials N --out runs [--velme PATH] [--jobs 8] [--no-prompt-cache]

Each trial gets a fresh project dir (the program's .velme file + velme.toml) and a fresh isolated home (HOME and the
XDG dirs, so no user config and no WASM cache is shared). The API key is taken from the environment (ANTHROPIC_API_KEY
or VELME_API_KEY) and never written anywhere. Output: <out>/<program>/velme/<model>/trial-K/{results.jsonl,meta.json}.

`cache_read`/`cache_write` come from the synth log's `cache_read_tokens`/`cache_write_tokens`. Its `tokens_in` is the
provider's `input_tokens`, which excludes cached tokens, so every arm counts uncached input there and cache apart.
"""
import argparse
import os
import shutil
import subprocess
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from common import HERE, dumps, load_inputs, load_program, loads, prices, trial_dir, write_trial

DEFAULT_VELME = HERE.parent.parent / "target/release/velme"


def parse_run(stdout, exit_code):
    """(output, error) from one `velme run --json` call: the goal's result value, or the failure's codes."""
    try:
        env = loads(stdout)
    except ValueError:
        return None, f"exit {exit_code}: unparseable stdout"
    res = (env.get("results") or [{}])[0]
    if res.get("status") == "ok" and exit_code == 0:
        return res.get("result"), None
    diags = res.get("diagnostics", []) + env.get("diagnostics", [])
    codes = ",".join(sorted({d.get("code", "?") for d in diags})) or "no diagnostic"
    return None, f"exit {exit_code}: {codes}"


def read_synth_log(project):
    log = project / ".velme/synth-log.jsonl"
    lines = [loads(line) for line in log.read_text().splitlines() if line.strip()] if log.exists() else []
    return {
        "tokens_in": sum(line.get("tokens_in") or 0 for line in lines),
        "tokens_out": sum(line.get("tokens_out") or 0 for line in lines),
        "cache_read": sum(line.get("cache_read_tokens") or 0 for line in lines),
        "cache_write": sum(line.get("cache_write_tokens") or 0 for line in lines),
        "calls": len(lines),
        "failed_attempts": sum(line.get("outcome") != "ok" for line in lines),
        "retries": sum(line.get("attempt", 0) > 0 for line in lines),
    }


def trial(args, program, k):
    cases = load_inputs(program["id"])
    source = Path(program["source"]).name
    with tempfile.TemporaryDirectory(prefix="velme-p1-") as tmp:
        tmp = Path(tmp)
        project, home = tmp / "project", tmp / "home"
        project.mkdir()
        home.mkdir()
        (project / source).write_text((HERE / program["id"] / program["source"]).read_text())
        toml = f'[synthesis]\nprovider = "anthropic"\nmodel = "{args.model}"\n'
        if args.no_prompt_cache:
            toml += "prompt_cache = false\n"
        if args.reply_format:
            toml += f'reply_format = "{args.reply_format}"\n'
        (project / "velme.toml").write_text(toml)
        env = {**os.environ, "HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config"), "XDG_CACHE_HOME": str(home / ".cache")}

        def velme(*a, cwd=project):
            return subprocess.run([str(args.velme), *a], cwd=cwd, env=env, capture_output=True, text=True, timeout=1800)

        t0 = time.monotonic()
        try:
            b = velme("build", source, "--json")
        except subprocess.TimeoutExpired:
            b = subprocess.CompletedProcess([], -1)  # a build that never finished is a failed build
        meta = {"program": program["id"], "arm": "velme", "model": args.model, "trial": k,
                "build_ok": b.returncode == 0, "build_exit": b.returncode,
                "wall_s": round(time.monotonic() - t0, 2), **read_synth_log(project)}
        keep = trial_dir(args.out, program["id"], "velme", args.model, k) / "build"  # what the build said, for diagnosis
        keep.mkdir(parents=True, exist_ok=True)
        (keep / "stdout.json").write_text(b.stdout or "")
        (keep / "stderr.txt").write_text(b.stderr or "")
        for f in (project / ".velme" / "synth-log.jsonl", project / "velme.lock"):
            if f.is_file():
                shutil.copy(f, keep / f.name)
        if not meta["build_ok"]:
            rows = [{"id": c["id"], "output": None, "error": "build failed"} for c in cases]
            return rows, meta

        def one(c):
            f = tmp / f"in-{c['id']}.json"
            f.write_text(dumps({x: c[x] for x in program["inputs"]}))
            try:
                r = velme("run", source, "--goal", program["goal"], "--input", str(f), "--json", "--locked", "--offline")
            except subprocess.TimeoutExpired:
                return {"id": c["id"], "output": None, "error": "timeout"}
            out, err = parse_run(r.stdout, r.returncode)
            return {"id": c["id"], "output": out, "error": err}

        with ThreadPoolExecutor(args.jobs) as pool:
            rows = list(pool.map(one, cases))
        return rows, meta


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--program", required=True, help="folder name under benchmark/p1, e.g. 0087")
    ap.add_argument("--model", required=True)
    ap.add_argument("--trials", type=int, default=1)
    ap.add_argument("--out", required=True, help="run root; trials go to <out>/<program>/velme/<model>/trial-N")
    ap.add_argument("--velme", type=Path, default=DEFAULT_VELME)
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--no-prompt-cache", action="store_true", help="velme.toml prompt_cache = false, so the log's tokens_in is the full input")
    ap.add_argument("--reply-format", choices=["ir-json", "notation"], help="velme.toml reply_format (default: Velme's own)")
    args = ap.parse_args()
    prices(args.model)  # fail before paying for a run whose cost cannot be reported
    program = load_program(args.program)
    for k in range(1, args.trials + 1):
        rows, meta = trial(args, program, k)
        write_trial(trial_dir(args.out, program["id"], "velme", args.model, k), rows, meta)
        ok = sum(r["error"] is None for r in rows)
        print(f"trial {k}: build_ok={meta['build_ok']} calls={meta['calls']} tokens={meta['tokens_in']}/{meta['tokens_out']} "
              f"cache={meta['cache_read']}/{meta['cache_write']} wall={meta['wall_s']}s ran_ok={ok}/{len(rows)}")


if __name__ == "__main__":
    main()
