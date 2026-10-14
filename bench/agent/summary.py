"""Pass rates and costs per condition from WORK/graded.json (judge verdicts).
usage: summary.py WORK [REPS, e.g. 2,3]"""
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

B = Path(sys.argv[1])
reps = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else None
rows = [r for r in json.load(open(B / "graded.json")) if reps is None or str(r["rep"]) in reps]
order = ["none", "claude-md", "baseline", "memctrl", "claude-md-scale", "memctrl-scale"]
conds = sorted({r["cond"] for r in rows}, key=lambda c: (order.index(c) if c in order else len(order), c))
tasks = list(dict.fromkeys(r["task"] for r in rows))
kinds = {t["id"]: t["kind"] for t in json.load(open(Path(__file__).resolve().parent / "tasks.json"))}
cell = defaultdict(list)
for r in rows:
    cell[(r["task"], r["cond"])].append(r)


def mark(rs):
    if not rs:
        return "-"
    return "".join("✓" if r.get("judge_pass") else ("?" if r.get("judge_pass") is None else "✗") for r in rs)


print(f"{'task':18} {'kind':14} " + " ".join(f"{c:>14}" for c in conds))
for t in tasks:
    print(f"{t:18} {kinds[t]:14} " + " ".join(f"{mark(cell[(t, c)]):>14}" for c in conds))
print()
for c in conds:
    rs = [r for r in rows if r["cond"] == c]
    p = sum(1 for r in rs if r.get("judge_pass"))
    rp = sum(1 for r in rs if r.get("rules_pass"))
    print(f"{c:15} judge {p}/{len(rs)} ({p / len(rs):.0%})  rules {rp}/{len(rs)}  "
          f"cost ${sum(r['cost_usd'] or 0 for r in rs):.2f} (median ${statistics.median(r['cost_usd'] or 0 for r in rs):.3f})  "
          f"turns {statistics.mean(r['turns'] or 0 for r in rs):.1f}  "
          f"input tok {statistics.median(r['input_tokens'] for r in rs):,.0f}  "
          f"time {statistics.median(r['duration_s'] for r in rs):.0f}s  "
          f"memctrl calls {sum(r['memctrl_calls'] for r in rs)}")
