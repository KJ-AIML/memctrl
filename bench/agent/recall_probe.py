"""What the prompt hook adds, beyond the session brief, for each benchmark task and for everyday
prompts that have nothing to do with the stored memories. Run it inside a copy of a memctrl
template, never the template itself: queries there would change what later runs see.
usage: recall_probe.py MEMCTRL_BIN [LIMIT]"""
import json, subprocess, sys
from pathlib import Path
LIMIT = sys.argv[2] if len(sys.argv) > 2 else "5"
SUFFIX = ("\n\nYou're working on your own and can't ask me questions. "
          "If you decide not to do something as asked, say why in your final message.")
EVERYDAY = ["Rename the variable x to count in this function and keep behavior the same.",
    "Write a short haiku about autumn.", "Explain what this regex does: ^[a-z0-9_-]{3,16}$",
    "Add an .editorconfig file with 2-space indentation.", "Update the copyright year in the LICENSE file.",
    "Summarize what changed in the last commit.", "Bump vitest to the latest version and fix anything that breaks.",
    "Fix the typo in the comment at the top of server.ts.", "Add JSDoc comments to the functions in src/config.ts.",
    "Can you make the logs prettier when running locally?", "Refactor middleware.ts so the error handling is easier to read.",
    "Add a CONTRIBUTING.md with our branch naming rules.", "Why does TypeScript complain about implicit any in this file?",
    "ช่วยอธิบายโค้ดในไฟล์ server.ts ให้หน่อย", "Write unit tests for loadConfig covering the default values."]
mc = sys.argv[1]
brief = json.loads(subprocess.run([mc, "context", "--format", "json"], capture_output=True, text=True).stdout)
bids = {e["id"] for e in brief["entries"]}
def rec(p):
    r = json.loads(subprocess.run([mc, "recall", p, "--format", "json", "--limit", LIMIT], capture_output=True, text=True).stdout)
    return [e for e in r["entries"] if e["id"] not in bids]
total = 0
print("### benchmark tasks: what recall adds beyond the brief")
for t in json.load(open(Path(__file__).resolve().parent / "tasks.json")):
    es = rec(t["prompt"] + SUFFIX); total += len(es)
    print(f"{t['id']:18}", " | ".join(e["content"][:60] for e in es) or "(nothing)")
print(f"added {total} memories over 16 tasks\n\n### unrelated everyday prompts")
total = 0
for p in EVERYDAY:
    es = rec(p + SUFFIX); total += len(es)
    print(f"{p[:50]:52}", " | ".join(e["content"][:50] for e in es) or "(nothing)")
print(f"added {total} memories over {len(EVERYDAY)} unrelated prompts")
