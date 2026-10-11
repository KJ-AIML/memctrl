# MemCtrl 2.1 implementation plan

Status: implemented on branch `feat/memory-2.1` (see "Delivery" at the end). Not merged, tagged or released.
Source: research brief of 2026-10-12. It covered the market scan, risks of memory for developers, and probes of the 2.0.0 release binary.

## Goal

Keep 2.0's trust core: one binary, no required model, lifecycle and verification states, provenance and audit. Then fix the three weaknesses the probes measured:

1. **Agents must choose to call memctrl.** Agents skip optional lookups, so memory has to be loaded every session.
2. **Retrieval is weak.** It has no term weighting, fails on Chinese, Japanese and Thai, returns noise for generic queries, and its query time grows with the store.
3. **Memories don't know when code changed.** No local tool ties a memory to the code it describes.

Then make memory reviewable and shareable through git, defend the write path against poisoning, and ship an eval that measures all of this.

## Non-goals

- No required LLM or embedding model. No model downloads.
- No hosted service.
- No change to `schema_version`. All new tables are additive, so 2.0 binaries keep working on a 2.1 database. Any writer needs SQLite with FTS5, because the index triggers fire on every write; see MIGRATION.
- No crates.io publication, tag or release. Those stay maintainer actions per DISTRIBUTION.md.

## Phase 1: get loaded every session

| Item | Design |
| --- | --- |
| `memctrl context` | Builds a budgeted brief of eligible memories. Default layers are `project` and `user`; `--layer` repeats to override. `--budget` is in approximate tokens (default 1500): ASCII counts as ¼ token per char, everything else as 1, which is conservative for CJK and Thai. |
| Ranking | confidence × verification (supported 1.5, unverified 1.0, disputed 0.5) × claim type (decision 1.3, derived lesson 1.1, assertion 1.0, observation 0.9, hypothesis 0.6) × layer (project 1.2) × a small bonus for reinforcement (access count). Ties go to the newest. |
| Sections | Decisions, Facts and conventions, Lessons, Hypotheses, and "Check before trusting" (disputed or stale). Each entry is one line, truncated at 400 characters, with its short id. |
| Formats | `markdown` (default), `json`, and `hook`. The `hook` format emits Claude Code's `hookSpecificOutput.additionalContext` JSON. `--write FILE` upserts the brief between `<!-- memctrl:context:begin/end -->` markers, e.g. into AGENTS.md for Codex and Cursor. |
| Side effects | Records provenance with method `context_brief`, which logs which memories went into each session. Does **not** bump access counts, because a brief that reinforces itself would be a feedback loop. If the database is missing, it prints an empty brief and creates no files, so a user-level hook never litters other repos. |
| Installer fixes | Claude Code moves `.claude/agent/skills/` → `.claude/skills/`. Codex moves `.codex/skills/` → `.agents/skills/` (current Codex docs). Cursor stays `.cursor/skills/`. |
| `install --hook` | Claude only. Merges a SessionStart hook running `<abs memctrl> context --format hook` into `.claude/settings.json` (project) or `~/.claude/settings.json` (user). The merge is idempotent, keeps key order (serde_json `preserve_order`), writes atomically, and refuses to touch invalid JSON. |
| SKILL.md | Rewritten. It drops the "MANDATORY" shouting in favor of accurate guidance: use specific query terms, save decisions with rationale and citations, and handle correction, review, stale memories and export. |

## Phase 2: retrieve well (FTS5 + BM25)

| Item | Design |
| --- | --- |
| Index | Two external-content FTS5 tables over `memories`, both already compiled into the bundled SQLite (zero new dependencies). `memory_fts` uses `porter unicode61` on content and tags. `memory_fts_tri` uses `trigram` on content. Triggers keep them in sync, including writes from 2.0 binaries. The index is rebuilt when first created and by `memctrl maintenance`. |
| Query terms | The query is split by script. Space-delimited text becomes lower-cased words, with stop words removed. Thai, Lao, Khmer, Myanmar, CJK, kana and Hangul runs are kept whole, by Unicode block, so Thai tone marks stay attached. |
| Passes | 1. Porter: one indexed query per term; BM25 contributions are summed per memory. 2. Substring fallback: a term with zero porter hits is retried as a trigram substring. This covers Latin words glued to CJK, and `auth` → `authentication`. 3. Trigram: each spaceless run of 3 or more characters becomes an OR of its trigrams. 4. Runs of 1–2 characters use an escaped `LIKE`. |
| Aliases | Built-in synonym groups: db/database, auth/authentication, prod/production, k8s/kubernetes, repo/repository, config/configuration, env/environment, deps/dependencies, js/javascript, ts/typescript, pg/postgres/postgresql. Add your own with `.memoryrc` `[retrieval] aliases = [["orm","sqlx","diesel"]]`, or turn the built-ins off with `builtin_aliases = false`. |
| Score | ΣBM25 × (0.5 + 0.5 × coverage) × layer boost × memory confidence. Coverage is the fraction of query terms matched; a term matches if it or one of its aliases matched. A trigram run counts by its matched fraction, with a 25% floor. Results are gated at 35% of the top score, deduplicated by content, then cut to `top_k`. |
| Explainability | New `why` per fact: matched terms, alias expansions, passes used, coverage and score. It is also stored in provenance sources. The provenance method becomes `lexical_bm25`. |
| Confidence | Mean of memory confidence × coverage over the selected facts. |
| Cache | The key adds `RETRIEVAL_VERSION = "lexical-v1"` and an alias fingerprint. The eligible-set fingerprint comes from a light scan of ids and states that skips content and regex sanitizing. |
| Legacy | `keyword_retrieve` stays as an explicit legacy function. It keeps the Python-oracle parity test and serves as the eval baseline. |

## Phase 3: tie memories to code

| Item | Design |
| --- | --- |
| Table | `memory_citations(id, memory_id, path, line_start, line_end, file_hash, span_hash, revision, created_at, refreshed_at)`. Hashes are SHA-256 over normalized text: LF line endings, trailing whitespace trimmed. |
| Root | `.memoryrc` `[citations] root` (relative to the rc file), else `MEMCTRL_PROJECT_ROOT`, else the nearest ancestor with `.git`, else the cwd. Paths are stored relative to the root with `/`. Absolute paths, `..` and symlinks escaping the root are rejected. |
| Freshness | `fresh`, `moved` (the same span was found elsewhere in the file), `changed`, `missing`, `unresolved` (outside the root) or `unchecked` (over 8 MB or not a file). A memory is `stale` if any citation is changed or missing, and `uncited` if it has none. |
| Where freshness shows | `why.freshness` on query results (computed after the cache, so it is never served stale), "Check before trusting" in the brief, and `show`. Ranking ignores file state, which keeps cache identity sound. |
| CLI | `add --cite path[:L[-M]]` (repeatable), `cite <id> <spec>…`, `stale [--layer]`, `refresh <id>`. `refresh` re-hashes after a human re-checks the memory and is audited as `citation_refresh`. |
| MCP | `memctrl_add` accepts `cite`, `claim_type` and `tags`. New tools: `memctrl_context` and `memctrl_stale`. |

## Phase 4: reviewable, safe writes

| Item | Design |
| --- | --- |
| Write guard | Runs on CLI and MCP `add`, LLM extraction, trigger extraction, the agent node and import. Flags: bidi controls and tag characters, zero-width characters (ZWJ/ZWNJ only when hidden between ASCII), prompt-injection phrasing ("ignore previous instructions", "don't tell the user", fake `<system>` tags, `curl … \| sh`, …), and an untrusted source in strict mode. A flagged memory becomes a `candidate` tagged `guard:<flag>`, and `add` reports `status: quarantined` with the flags. |
| Policy | `.memoryrc` `[guard] mode = "flag"` (default) \| `"strict"` \| `"off"`, and `trusted_sources = ["manual","user","cli"]`. |
| `memctrl export` | Writes `.memctrl/memory.md`, defaulting to the `project` layer. Entries are sorted deterministically by section, created_at and id. Each entry is a bullet with readable content plus a metadata comment containing id, states, citations and a content hash. Volatile fields such as access counts stay out to keep diffs clean. `--check` exits nonzero when the file is out of date. Export also writes `.memctrl/.gitignore`, which ignores the database files so the folder can be committed. |
| `memctrl import` | Atomic. A new id is inserted with its original identity and states. An existing id is never overwritten: identical entries are skipped, differing ones keep the local version. Entries whose hash doesn't match their content were edited by hand and become new candidates tagged `import:edited`. The guard runs on everything. `--dry-run` prints the plan. |

## Phase 5: prove it

| Item | Design |
| --- | --- |
| `memctrl eval` | Runs in an in-memory store and never opens your database. The built-in suite `assets/eval/dev-memory.json` contains developer memories and queries across English stems, identifiers, aliases, Thai, Chinese, Japanese, paraphrases (expected to stay hard without embeddings) and negatives. `--file` runs your own suite. |
| Metrics | Per retriever (`lexical`, `legacy` or `both`): hit@1, hit@k, recall@k, MRR, precision@k, negative accuracy, facts and approximate tokens returned, p50/p95 latency, a per-kind breakdown and failures. Also reports brief size. `--min-hit-rate` makes it a CI gate. |
| Test | `tests/eval.rs` asserts that lexical beats legacy on the built-in suite and meets the published floor. |

## Compatibility and docs

- MIGRATION.md covers the provenance method rename (`keyword_tree` → `lexical_bm25`), the confidence semantics, the installer paths, the additive tables and the guard quarantine behavior.
- README gains the new commands and config. RELEASE_NOTES.md moves to 2.1.0, with the 2.0.0 notes archived in `docs/history`.
- Version: `2.1.0`.

## Verification

`make ci` must pass after each phase: fmt, clippy `-D warnings`, all tests and the release build. Phase-specific regression contracts:
- the brief respects the budget and never bumps access
- missing database means no files are created
- the hook merge is idempotent and keeps unrelated keys
- Chinese, Japanese and Thai are found
- generic queries no longer return everything
- freshness is correct across edit, move and delete
- guard quarantine
- the export → import round trip is lossless, and edited entries become candidates
- eval floors are met

## Delivery

Commits on `feat/memory-2.1`, one per phase, each passing `make ci`:

| Phase | Commit | Result |
| --- | --- | --- |
| 1 | `feat(context)` | `context`, `install --hook`, fixed skill paths, `memctrl_context` |
| 2 | `feat(search)` | FTS5 BM25 + trigrams, aliases, `why`; about 10 ms at 3,000 memories (from about 120 ms) |
| 3 | `feat(citations)` | `--cite`, `cite`, `stale`, `refresh`, freshness everywhere memories are used, `memctrl_stale` |
| 4 | `feat(guard,mirror)` | write guard on all write paths; `export` / `import`, byte-identical round trip |
| 5 | `feat(eval)` | `memctrl eval`; hit@5 0.927 vs 0.756 (2.0), negatives 0.857 vs 0.429, CJK and Thai 10/10 vs 2/10 |
| 6 | `docs(release)` | README, MIGRATION, release notes, version 2.1.0 |

Deviations from the plan:
- Freshness is not part of ranking; it only labels results. This keeps cache identity independent of the working tree.
- Hand-written bullets in an export import as candidates, filed under the section heading they sit beneath. This came out of designing import and makes the file a review channel for new memories.
- Citation hashes are a 128-bit SHA-256 prefix, because full 256-bit hex is redacted by output sanitizing.
