"""Run every task in every condition as a fresh, isolated Claude Code session (`claude -p`).

Each run gets its own copy of the condition's template. User settings, plugins, auto memory
and MCP servers are not loaded, so the only memory the agent has is what the condition gives
it. The same two sentences follow every task, telling the agent it works alone.
usage: run.py WORK REP [--conds a,b] [--only cond/task,...] [--jobs N] [--model sonnet]
"""
import argparse
import json
import os
import re
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
ap = argparse.ArgumentParser()
ap.add_argument("work", type=Path)
ap.add_argument("rep")
ap.add_argument("--conds", default="", help="default: every template in WORK/templates")
ap.add_argument("--only", default="")
ap.add_argument("--jobs", type=int, default=4)
ap.add_argument("--model", default="sonnet")
ap.add_argument("--claude", default=str(Path.home() / ".local/bin/claude"))
a = ap.parse_args()
W = a.work.resolve()

SUFFIX = ("\n\nYou're working on your own and can't ask me questions. "
          "If you decide not to do something as asked, say why in your final message.")
ALLOWED = ["Read", "Edit", "Write", "Skill"] + [f"Bash({c} *)" for c in (
    "ls", "cat", "grep", "rg", "find", "head", "tail", "wc", "sed -n", "tree", "node",
    "git log", "git diff", "git show", "git status", "git blame", "git grep", "mkdir", "memctrl")]
BASE_PATH = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
EXCLUDE = [":(exclude).memctrl", ":(exclude).claude", ":(exclude)CLAUDE.md", ":(exclude).memoryrc"]
MEMCTRL_CALL = re.compile(r"(^|[\s;&|(])memctrl\s+[a-z]")


def git(repo, *args):
    return subprocess.run(["git", *args], cwd=repo, capture_output=True, text=True).stdout


def one(cond, task):
    import shutil

    out = W / "runs" / f"r{a.rep}" / cond / task["id"]
    if (out / "result.json").exists():
        return f"skip {cond}/{task['id']}"
    if out.exists():
        # An interrupted run: set it aside rather than delete it.
        out.rename(out.with_name(out.name + f".stale-{int(time.time())}"))
    out.mkdir(parents=True)
    repo = out / "repo"
    shutil.copytree(W / "templates" / cond, repo, symlinks=True)
    bindir = W / f"bin-{cond}"
    env = {"HOME": os.environ["HOME"], "USER": os.environ.get("USER", ""), "LANG": "en_US.UTF-8", "TERM": "dumb",
           "PATH": (f"{bindir}:" if bindir.is_dir() else "") + BASE_PATH}
    cmd = [a.claude, "-p", task["prompt"] + SUFFIX, "--model", a.model,
           "--setting-sources", "project,local", "--strict-mcp-config", "--no-session-persistence",
           "--output-format", "stream-json", "--verbose", "--include-hook-events",
           "--permission-prompts", "none", "--permission-mode", "acceptEdits",
           "--tools", "Bash,Read,Edit,Write,Skill", "--allowedTools", *ALLOWED,
           "--settings", json.dumps({"autoMemoryEnabled": False}), "--max-budget-usd", "2"]
    started = time.time()
    with open(out / "transcript.jsonl", "w") as f:
        p = subprocess.run(cmd, cwd=repo, env=env, stdout=f, stderr=subprocess.PIPE, text=True, timeout=1500)
    wall = time.time() - started
    git(repo, "add", "-A", "-N")
    (out / "diff.patch").write_text(git(repo, "diff", "HEAD", "--", ".", *EXCLUDE))
    result, tools, memctrl_calls, injected = {}, 0, 0, []
    for line in open(out / "transcript.jsonl"):
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if ev.get("type") == "result":
            result = ev
        if ev.get("type") == "system" and ev.get("subtype") == "hook_response" and ev.get("output", "").strip():
            injected.append(ev.get("hook_event"))
        if ev.get("type") == "assistant":
            for c in ev["message"].get("content", []):
                if c.get("type") == "tool_use":
                    tools += 1
                    inp = c.get("input", {})
                    if c["name"] == "Bash" and MEMCTRL_CALL.search(inp.get("command", "")):
                        memctrl_calls += 1
                    if c["name"] == "Skill" and "memctrl" in json.dumps(inp):
                        memctrl_calls += 1
    u = result.get("usage", {})
    summary = {
        "cond": cond, "task": task["id"], "rep": a.rep, "rc": p.returncode, "stderr": p.stderr[-500:],
        "final": result.get("result", ""), "is_error": result.get("is_error"),
        "cost_usd": result.get("total_cost_usd"), "turns": result.get("num_turns"), "duration_s": round(wall, 1),
        "tool_calls": tools, "memctrl_calls": memctrl_calls, "hooks_injected": injected,
        "input_tokens": u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0)
        + u.get("cache_creation_input_tokens", 0),
        "output_tokens": u.get("output_tokens", 0), "denials": len(result.get("permission_denials", [])),
    }
    json.dump(summary, open(out / "result.json", "w"), ensure_ascii=False, indent=1)
    return (f"{cond:16} {task['id']:18} rc={p.returncode} ${summary['cost_usd']} turns={summary['turns']} "
            f"tools={tools} memctrl={memctrl_calls} {wall:.0f}s")


def main():
    tasks = json.load(open(HERE / "tasks.json"))
    conds = [c for c in a.conds.split(",") if c] or sorted(p.name for p in (W / "templates").iterdir() if p.is_dir())
    only = set(filter(None, a.only.split(",")))
    jobs = [(c, t) for t in tasks for c in conds if not only or f"{c}/{t['id']}" in only]
    with ThreadPoolExecutor(a.jobs) as pool:
        for line in pool.map(lambda ct: one(*ct), jobs):
            print(line, flush=True)


if __name__ == "__main__":
    main()
