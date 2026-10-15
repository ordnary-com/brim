#!/usr/bin/env python3
"""Benchmarks: compile time, native run time and interpreter run time of the
programs in benches/, the best of several runs each.

    scripts/bench.py                  # build brim (release, LLVM) and run all
    scripts/bench.py fib lists        # only these
    scripts/bench.py --save base.json # keep the results
    scripts/bench.py --compare base.json

Needs LLVM 22 for the native backend (LLVM_SYS_221_PREFIX) and a C compiler.
"""

import json
import os
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUNS = 3


def best(command, runs=RUNS):
    times = []
    for _ in range(runs):
        start = time.perf_counter()
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        times.append(time.perf_counter() - start)
    return min(times)


def main():
    args = sys.argv[1:]
    save = compare = None
    if "--save" in args:
        save = args[args.index("--save") + 1]
        del args[args.index("--save") : args.index("--save") + 2]
    if "--compare" in args:
        compare = args[args.index("--compare") + 1]
        del args[args.index("--compare") : args.index("--compare") + 2]
    subprocess.run(
        ["cargo", "build", "-q", "--release", "--features", "llvm", "-p", "brim-cli"],
        cwd=ROOT,
        check=True,
    )
    brim = os.path.join(ROOT, "target", "release", "brim")
    names = sorted(f[:-5] for f in os.listdir(os.path.join(ROOT, "benches")) if f.endswith(".brim"))
    if args:
        names = [n for n in names if n in args]
    baseline = json.load(open(compare)) if compare else {}
    results = {}
    out = tempfile.mkdtemp(prefix="brim-bench-")
    print(f"{'program':<10} {'compile':>9} {'native':>9} {'interp':>9}")
    for name in names:
        source = os.path.join(ROOT, "benches", name + ".brim")
        exe = os.path.join(out, name)
        compile_time = best([brim, "build", "--release", source, "-o", exe], runs=1)
        native = best([exe])
        interp = best([brim, "run", source])
        results[name] = {"compile": compile_time, "native": native, "interp": interp}
        line = f"{name:<10} {compile_time:>8.3f}s {native:>8.3f}s {interp:>8.3f}s"
        if name in baseline:
            changes = []
            for key in ("compile", "native", "interp"):
                before = baseline[name][key]
                if before > 0:
                    changes.append(f"{key} {100 * (results[name][key] - before) / before:+.0f}%")
            line += "   " + ", ".join(changes)
        print(line)
    if save:
        with open(save, "w") as f:
            json.dump(results, f, indent=2)


if __name__ == "__main__":
    main()
