# MemCtrl 2.1.0: memory that loads itself, searches in any language, and knows when code changed

2.1 keeps 2.0's trust core: one binary, no required model, lifecycle and verification states, provenance and audit. It fixes what probes of 2.0.0 measured: agents had to remember to ask, retrieval failed on Chinese, Japanese and Thai and answered generic questions with noise, and nothing told you when a memory's code had changed.

## Highlights

- **Loaded every session.** `memctrl context` builds a budgeted brief of the highest-trust memories. `memctrl install --tool claude --hook` injects it at every Claude Code session start, and also after `/clear`, resume and compaction. `memctrl context --write AGENTS.md` serves Codex, Cursor and other hosts. The brief never counts as access, and where no store exists it does nothing.
- **Real search.** Retrieval uses SQLite FTS5: BM25 over stemmed words, plus trigrams for scripts written without spaces. There are no new dependencies, because FTS5 was already compiled in. Every fact explains itself (`why`: matched terms, aliases, coverage). Built-in synonym groups ship, and you can add your own. Queries take about 10 ms on a 3,000-memory store, down from about 120 ms.
- **Memories tied to code.** `--cite path:12-40` hashes the cited lines. Query results, the brief, `show` and `memctrl stale` flag memories whose code changed or vanished; moved code is followed. `memctrl refresh` re-anchors after review and is audited.
- **Poisoning resistance.** Hidden Unicode or instruction-like text ("ignore previous instructions", `curl … | sh`) turns a new memory into a candidate for review, on every write path. A strict mode also holds untrusted sources.
- **Shared through git.** `memctrl export` writes a deterministic Markdown file for pull-request review, and `memctrl import` brings it into another store without overwriting local state. Edited entries and new bullets written by teammates arrive as candidates.
- **Measured.** `memctrl eval` runs a developer-memory suite in memory. On the built-in suite: hit@5 0.927 vs 0.756 for the 2.0 scorer, unanswerable queries correctly empty 0.857 vs 0.429, Thai/Chinese/Japanese/Korean 10/10 vs 2/10. Paraphrases with no shared words remain a known limit (3/6).
- **Fixed:** the installer now writes skills where tools read them. Claude Code uses `.claude/skills/` (2.0 wrote `.claude/agent/skills/`). Codex uses `.agents/skills/` (2.0 wrote `.codex/skills/`).

## Upgrade notes

Read the 2.0 → 2.1 section of [MIGRATION.md](MIGRATION.md). In short:
- The database schema is extended additively and indexed on first open.
- The provenance method for local retrieval is now `lexical_bm25`.
- Retrieval scores and `confidence` follow the new ranking.
- Re-run `memctrl install` to put the skill in the right place.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](docs/history).
