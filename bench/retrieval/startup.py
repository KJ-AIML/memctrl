"""Median wall time of common commands (process start included), 31 runs each."""
import subprocess, sys, time
m, name = sys.argv[1], sys.argv[2]
def t(args):
    xs = []
    for _ in range(31):
        s = time.perf_counter(); subprocess.run(args, capture_output=True); xs.append((time.perf_counter() - s) * 1000)
    xs.sort(); return round(xs[15], 1)
cmds = {"version": ["--version"], "list": ["list", "--limit", "1"], "query": ["query", "make test-int"], "context": ["context"]}
print(f"{name:10}", " | ".join(f"{k} {t([m, *v])}" for k, v in cmds.items()))
