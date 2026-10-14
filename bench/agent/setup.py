"""Build the starting points of the agent benchmark in WORK/templates.

none        the ledger-api fixture: code and git history only
claude-md   every memory written into CLAUDE.md, the way most teams keep notes today
memctrl     the same memories in a memctrl store, set up with `memctrl install --tool claude --hook`
            using --memctrl (the build under test)
baseline    the same, set up by an older release (--baseline), when given

With --scale N, also claude-md-scale and memctrl-scale: the same plus N memories about other
parts of a larger system (distractors.py), none of which bears on the tasks.

usage: setup.py WORK --memctrl PATH [--baseline PATH] [--scale N]
"""
import argparse
import json
import os
import shutil
import subprocess
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURE = HERE / "fixture"
GIT = ["git", "-c", "user.name=bench", "-c", "user.email=bench@example.invalid", "-c", "commit.gpgsign=false"]
ENV = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": os.environ["HOME"]}


def run(cmd, cwd):
    p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, env=ENV)
    if p.returncode != 0:
        raise SystemExit(f"{cmd} failed in {cwd}:\n{p.stdout}\n{p.stderr}")
    return p.stdout


def clone(work, name):
    dst = work / "templates" / name
    if dst.exists():
        raise SystemExit(f"{dst} exists; use a new WORK folder")
    run(["git", "clone", "-q", str(FIXTURE / "ledger-api.bundle"), str(dst)], work)
    run(["git", "remote", "remove", "origin"], dst)
    return dst


def with_memctrl(work, name, binary):
    """A template whose store holds the fixture memories, set up by `binary`."""
    bindir = work / f"bin-{name}"
    bindir.mkdir(parents=True)
    shutil.copy(binary, bindir / "memctrl")
    repo = clone(work, name)
    (repo / ".memctrl").mkdir()
    shutil.copy(FIXTURE / "memories.sqlite3", repo / ".memctrl" / "memories.db")
    (repo / ".memctrl" / ".gitignore").write_text("*\n")
    shutil.copy(FIXTURE / "memoryrc", repo / ".memoryrc")
    run([str(bindir / "memctrl"), "install", "--tool", "claude", "--project", "--hook"], repo)
    (repo / ".git" / "info" / "exclude").write_text(".claude/settings.local.json\n")
    run([*GIT, "add", "-A"], repo)
    run([*GIT, "commit", "-qm", "chore: memctrl setup"], repo)
    return repo


def claude_md(mems):
    groups = [
        ("Decisions", lambda x: x["layer"] == "project" and x["claim_type"] == "decision"),
        ("Facts and conventions", lambda x: x["layer"] == "project" and x["claim_type"] in ("assertion", "observation")
         and x["verification_state"] != "disputed"),
        ("Open hypotheses", lambda x: x["layer"] == "project" and x["claim_type"] == "hypothesis"),
        ("Disputed", lambda x: x["layer"] == "project" and x["verification_state"] == "disputed"),
        ("Preferences", lambda x: x["layer"] == "user"),
        ("Current work", lambda x: x["layer"] == "session"),
    ]
    lines, used = ["# ledger-api project notes", ""], set()
    for title, pick in groups:
        rows = [x for x in mems if pick(x) and x["id"] not in used]
        if rows:
            lines += [f"## {title}", ""]
            for x in sorted(rows, key=lambda x: x["created_at"]):
                used.add(x["id"])
                lines.append(f"- {x['content']}")
            lines.append("")
    missing = [x["content"] for x in mems if x["id"] not in used]
    if missing:
        raise SystemExit(f"memories not placed in CLAUDE.md: {missing}")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("work", type=Path)
    ap.add_argument("--memctrl", required=True, type=Path)
    ap.add_argument("--baseline", type=Path)
    ap.add_argument("--scale", type=int, default=0)
    a = ap.parse_args()
    work = a.work.resolve()
    (work / "templates").mkdir(parents=True, exist_ok=True)

    m = with_memctrl(work, "memctrl", a.memctrl.resolve())
    mems = json.loads(run([str(work / "bin-memctrl" / "memctrl"), "list", "--limit", "5000"], m))["memories"]
    if a.baseline:
        with_memctrl(work, "baseline", a.baseline.resolve())

    c = clone(work, "claude-md")
    notes = claude_md(mems)
    (c / "CLAUDE.md").write_text(notes, encoding="utf-8")
    run([*GIT, "add", "-A"], c)
    run([*GIT, "commit", "-qm", "docs: project notes"], c)

    n = clone(work, "none")
    run([*GIT, "commit", "-q", "--allow-empty", "-m", "chore: setup"], n)

    if a.scale:
        extra = work / "distractors.jsonl"
        run(["python3", str(HERE / "distractors.py"), str(extra), str(a.scale)], work)
        rows = [json.loads(line) for line in open(extra, encoding="utf-8")]
        # CLAUDE.md: one section per component, the ledger-api notes kept whole among them.
        cs = clone(work, "claude-md-scale")
        by_component = defaultdict(list)
        for r in rows:
            by_component[r["component"]].append(r["content"])
        sections = {k: f"## {k}\n\n" + "\n".join(f"- {t}" for t in v) + "\n" for k, v in by_component.items()}
        sections["ledger-api"] = notes.replace("# ledger-api project notes", "## ledger-api").replace("\n## ", "\n### ")
        (cs / "CLAUDE.md").write_text("# Monorepo notes\n\n" + "\n".join(sections[k] for k in sorted(sections)),
                                      encoding="utf-8")
        run([*GIT, "add", "-A"], cs)
        run([*GIT, "commit", "-qm", "docs: monorepo notes"], cs)
        ms = with_memctrl(work, "memctrl-scale", a.memctrl.resolve())
        for r in rows:
            out = run([str(work / "bin-memctrl-scale" / "memctrl"), "add", r["content"], "--layer", r["layer"],
                       "--claim-type", r["claim_type"]], ms)
            if '"quarantined"' in out:
                raise SystemExit(f"held for review: {r['content']}")

    print(f"templates in {work / 'templates'}: " + ", ".join(sorted(p.name for p in (work / "templates").iterdir())))


if __name__ == "__main__":
    main()
