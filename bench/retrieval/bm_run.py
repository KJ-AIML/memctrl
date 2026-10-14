"""Run every suite through basic-memory (text, vector, hybrid) and save ranked keys per query.

Each memory becomes one note titled m001, m002, ... so titles carry no hint of the answer.
TOOLS_DIR holds a virtualenv with basic-memory installed (bm-venv); its HOME is TOOLS_DIR/bm-home.
usage: bm_run.py OUT_DIR TOOLS_DIR
"""
import json
import statistics
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

C, T = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()
SUITES_DIR = Path(__file__).resolve().parent / "suites"
BIN = str(T / "bm-venv" / "bin" / "basic-memory")
ENV = {"HOME": str(T / "bm-home"), "PATH": "/usr/bin:/bin", "COLUMNS": "200"}
PROJECT_OF = {"realdev": "realdev", "builtin": "builtin", "thai": "thai", "thai-heldout": "thai", "en-mirror": "enmirror"}


def bm(*args, check=True):
    p = subprocess.run([BIN, *args], env=ENV, capture_output=True, text=True)
    if check and p.returncode != 0:
        raise SystemExit(f"basic-memory {args[:3]} failed: {p.stderr[-400:]}")
    return p


def setup(suite, project):
    data = json.load(open(SUITES_DIR / f"{suite}.json", encoding="utf-8"))
    root = T / "bm-data" / project
    mapping_path = T / "bm-data" / f"{project}.map.json"
    if mapping_path.exists() and project != "builtin":
        return json.load(open(mapping_path))
    root.mkdir(parents=True, exist_ok=True)
    mapping = {}
    for i, m in enumerate(data["memories"], 1):
        name = f"m{i:03d}"
        mapping[name] = m["key"]
        (root / f"{name}.md").write_text(f"---\ntitle: {name}\ntype: note\n---\n\n{m['content']}\n", encoding="utf-8")
    json.dump(mapping, open(mapping_path, "w"))
    if project != "builtin":
        bm("project", "add", project, str(root))
    bm("reindex", "--project", project)
    return mapping


def search(project, mapping, query, flag):
    args = ["tool", "search-notes", query, "--json", "--project", project]
    if flag:
        args.append(flag)
    t = time.perf_counter()
    p = bm(*args, check=False)
    ms = (time.perf_counter() - t) * 1000
    if p.returncode != 0:
        return {"query": query, "got": [], "error": p.stderr.strip()[-200:], "ms": ms}
    try:
        results = json.loads(p.stdout)["results"]
    except (ValueError, KeyError):
        return {"query": query, "got": [], "error": "unparseable output: " + p.stdout[:120], "ms": ms}
    got = []
    for r in results:
        key = mapping.get(r.get("title"))
        if key and key not in got:
            got.append(key)
    return {"query": query, "got": got, "error": "", "ms": ms}


def main():
    (C / "out").mkdir(parents=True, exist_ok=True)
    maps = {}
    for suite, project in PROJECT_OF.items():
        maps[suite] = setup(suite, project)
    for mode, flag in (("text", None), ("vector", "--vector"), ("hybrid", "--hybrid")):
        if mode == "text":
            bm("config", "set", "default_search_type", "text")
        try:
            for suite, project in PROJECT_OF.items():
                cases = json.load(open(SUITES_DIR / f"{suite}.json", encoding="utf-8"))["cases"]
                with ThreadPoolExecutor(6) as pool:
                    rows = list(pool.map(lambda c: search(project, maps[suite], c["query"], flag), cases))
                json.dump(rows, open(C / "out" / f"bm-{suite}-{mode}.json", "w"), ensure_ascii=False, indent=1)
                errs = sum(1 for r in rows if r["error"])
                print(f"{mode:6} {suite:13} queries={len(rows)} errors={errs} p50_ms={statistics.median(r['ms'] for r in rows):.0f}",
                      flush=True)
        finally:
            if mode == "text":
                bm("config", "unset", "default_search_type")


if __name__ == "__main__":
    main()
