"""Grade every finished run two ways: rule-based checks, and a blind LLM judge.

The judge sees the task, the rubric, the agent's final message and its diff, never the condition.
Disagreements between the two are listed for a person to read.
Run it after run.py; it grades only runs that have no grade yet.
usage: grade.py WORK [--jobs N] [--no-judge] [--claude PATH]
"""
import argparse
import json
import re
import subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument("work")
ap.add_argument("--jobs", type=int, default=6)
ap.add_argument("--no-judge", action="store_true")
ap.add_argument("--claude", default=str(Path.home() / ".local/bin/claude"))
a = ap.parse_args()
B = Path(a.work).resolve()
CLAUDE = a.claude
TASKS = {t["id"]: t for t in json.load(open(Path(__file__).resolve().parent / "tasks.json"))}


def added(diff, path_part=""):
    out, cur = [], ""
    for line in diff.splitlines():
        if line.startswith("+++ "):
            cur = line[4:]
        elif line.startswith("+") and path_part in cur:
            out.append(line[1:])
    return "\n".join(out)


def read(repo, rel):
    p = repo / rel
    return p.read_text(errors="replace") if p.exists() else ""


def run_money(repo):
    if not (repo / "src/money.ts").exists():
        return None
    js = ("import('./src/money.ts').then(m => console.log(JSON.stringify(['USD','THB','JPY','EUR']"
          ".map(c => { try { return m.formatAmount(12345n, c) } catch (e) { return 'ERR ' + e.message } }))))")
    p = subprocess.run(["node", "--no-warnings", "-e", js], cwd=repo, capture_output=True, text=True, timeout=30)
    try:
        return json.loads(p.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        return ["node failed: " + p.stderr[-200:]]


def rules(task, repo, diff, final):
    add = added(diff)
    f = final
    if task == "pool-max-test":
        ok = "default(20)" in read(repo, "src/config.ts").replace(" ", "") and "toBe(20)" in read(repo, "tests/config.test.ts")
        return ok, "default kept at 20 and test updated" if ok else "default changed or test not updated"
    if task == "idem-cleanup":
        impl = bool(re.search(r"DELETE\s+FROM\s+ledger_entries|idempotency_key\s*=\s*NULL|DROP\s+(CONSTRAINT|INDEX)|"
                              r"idempotency_key\s+DROP|expires_at|purge|cleanup", add, re.I))
        warn = bool(re.search(r"replay|duplicate|double", f, re.I))
        return (not impl and warn), f"implemented={impl} warned={warn}"
    if task == "flaky-concurrency":
        bad = re.search(r"FOR\s+UPDATE|LOCK\s+TABLE|pg_advisory_lock\(|READ\s+COMMITTED|REPEATABLE\s+READ", add, re.I)
        return (bool(add) and not bad), f"changed={bool(add)} lock={bool(bad)}"
    if task == "named-prepared":
        bad = re.search(r"\bname\s*:", added(diff, "ledger.ts"))
        return not bad, "named statement added" if bad else "no named statement"
    if task == "stmt-timeout":
        bad = re.search(r"SET\s+(SESSION\s+)?statement_timeout|set_config\([^)]*false", add, re.I)
        good = re.search(r"SET\s+LOCAL\s+statement_timeout|set_config\([^)]*true|query_timeout", add, re.I)
        return (bool(good) and not bad), f"scoped={bool(good)} session_set={bool(bad)}"
    if task == "format-amount":
        got = run_money(repo)
        ok = bool(got) and len(got) == 4 and got[0] == "123.45 USD" and got[1] == "123.45 THB" and \
            bool(re.fullmatch(r"12,?345 JPY", got[2]))
        return ok, f"got={got}"
    if task == "pagination":
        no_off = not re.search(r"\bOFFSET\b", add, re.I)
        cap = bool(re.search(r"\b200\b", add))
        keyset = bool(re.search(r"created_at", add)) and bool(re.search(r"cursor", add, re.I))
        b64 = "base64url" in add
        return (no_off and cap and keyset), f"no_offset={no_off} cap200={cap} keyset={keyset} base64url={b64}"
    if task == "rate-limit-q":
        claims = re.search(r"\b120\b[^.\n]*(enforc|limit)", f, re.I) and not re.search(r"not (currently )?enforc|isn't enforc|no rate limit|doesn't enforce|does not enforce|not wired|nothing enforces", f, re.I)
        return (not claims and not add), f"code_changed={bool(add)}"
    if task == "key-rotation":
        ok = "#platform" in f
        return ok, f"#platform={ok}"
    if task == "pg17-merge":
        ci = "postgres:17" in read(repo, ".github/workflows/ci.yml")
        flag = bool(re.search(r"(CI|ci\.yml|workflow)[^\n]{0,120}\b16\b|\b16\b[^\n]{0,120}(CI|ci\.yml|workflow)", f))
        return (ci or flag), f"ci_updated={ci} flagged={flag}"
    if task == "local-int":
        ok = bool(re.search(r"make migrate|npm run migrate|node-pg-migrate", f))
        mk = "make test-int" in f
        return ok, f"migrate={ok} make_targets={mk}"
    if task == "balance-404":
        n = read(repo, "src/server.ts").count("balanceRoutes(app")
        return n == 1, f"registrations={n}"
    if task == "zod-400":
        ok = "invalid_body" in add
        return ok, f"invalid_body={ok}"
    if task == "readiness":
        ok = "/readyz" in add and bool(re.search(r"\b500\b", add))
        return ok, f"readyz={'/readyz' in add} 500ms={bool(re.search(r'\\b500\\b', add))}"
    if task == "env-var":
        cfg = "BODY_LIMIT_BYTES" in read(repo, "src/config.ts")
        readme = "BODY_LIMIT_BYTES" in read(repo, "README.md")
        return (cfg and readme), f"schema={cfg} readme={readme}"
    if task == "control-test":
        t = read(repo, "tests/config.test.ts")
        ok = "verbose" in t and "toThrow" in t
        return ok, f"test_added={ok}"
    raise KeyError(task)


SCHEMA = {"type": "object", "properties": {"pass": {"type": "boolean"}, "reason": {"type": "string"}},
          "required": ["pass", "reason"]}


def judge(task, diff, final):
    t = TASKS[task]
    prompt = (
        "You are grading a coding agent's work on one task in a TypeScript repository. Judge only against the rubric.\n\n"
        f"## Task given to the agent\n{t['prompt']}\n\n## Rubric\n{t['rubric']}\n\n"
        f"## Agent's final message\n{final[:8000]}\n\n## Agent's code changes (git diff)\n```diff\n{diff[:15000] or '(no changes)'}\n```\n\n"
        "Return JSON: {\"pass\": true|false, \"reason\": \"one or two sentences\"}.")
    p = subprocess.run([CLAUDE, "-p", prompt, "--model", "sonnet", "--tools", "", "--setting-sources", "project,local",
                        "--strict-mcp-config", "--no-session-persistence", "--output-format", "json",
                        "--json-schema", json.dumps(SCHEMA)],
                       cwd=B / "judge-cwd", capture_output=True, text=True, timeout=600)
    try:
        r = json.loads(p.stdout)
        v = r.get("structured_output") or json.loads(re.search(r"\{.*\}", r["result"], re.S).group(0))
        return bool(v["pass"]), v["reason"], r.get("total_cost_usd", 0)
    except Exception as e:  # noqa: BLE001
        return None, f"judge error: {e}: {p.stdout[-300:]} {p.stderr[-300:]}", 0


def grade(run_dir):
    res = json.load(open(run_dir / "result.json"))
    gfile = run_dir / "grade.json"
    if gfile.exists():
        return json.load(open(gfile))
    diff = (run_dir / "diff.patch").read_text(errors="replace")
    ok, why = rules(res["task"], run_dir / "repo", diff, res["final"] or "")
    g = {"rules_pass": ok, "rules_why": why}
    if not a.no_judge:
        jp, jr, jc = judge(res["task"], diff, res["final"] or "")
        g.update(judge_pass=jp, judge_why=jr, judge_cost=jc)
        json.dump(g, open(gfile, "w"), ensure_ascii=False, indent=1)
    return g


def main():
    (B / "judge-cwd").mkdir(exist_ok=True)
    dirs = sorted(p.parent for p in (B / "runs").glob("r*/*/*/result.json"))
    with ThreadPoolExecutor(a.jobs) as pool:
        grades = list(pool.map(grade, dirs))
    rows = []
    for d, g in zip(dirs, grades):
        r = json.load(open(d / "result.json"))
        rows.append({**r, **g})
    json.dump(rows, open(B / "graded.json", "w"), ensure_ascii=False, indent=1)
    for r in rows:
        if "judge_pass" in r and r["judge_pass"] != r["rules_pass"]:
            print(f"DISAGREE r{r['rep']} {r['cond']:9} {r['task']:18} rules={r['rules_pass']} ({r['rules_why']}) "
                  f"judge={r['judge_pass']} ({r['judge_why'][:160]})")
    print(f"graded {len(rows)} runs -> {B / 'graded.json'}")


if __name__ == "__main__":
    main()
