# Full benchmark of 2.6.1

Everything measured again for the 2.6.1 release binaries, against 2.5.0, on 2026-10-17: the agent benchmark (both task sets, two rounds, a store grown to 1,069 memories), the retrieval suites, and stability and speed. Raw numbers, including every run with the judge's reason, are in `release-2.6.json`.

**In short:**
- **Same accuracy as CLAUDE.md, for less.** On the 16 standard tasks, memctrl 2.6.1 got 31 of 32 right, the same as all 70 notes pasted into CLAUDE.md, at a median $0.047 per task against $0.068. 2.5.0 got 28, and no memory 19.
- **At 1,069 memories, memctrl is ahead.** memctrl 2.6.1 got 32 of 32 at $0.047 a task. CLAUDE.md got 28 of 32 at $0.231, because every session reads all 1,069 notes.
- **Reworded requests:** 2.6.1 got 13 of 16, 2.5.0 11, CLAUDE.md 15, no memory 7.
- **Nothing else changed.** Retrieval results are identical to 2.5.0. Speed is the same, and 60 seconds of concurrent writes and 30 kills mid-write lost nothing.

## Agent benchmark

The setup is described in [agent-benchmark.md](agent-benchmark.md): the ledger-api project, 70 memories its tester saved, and Claude Code (2.1.301, `claude -p --model sonnet`) in fresh isolated sessions. A blind Sonnet judge grades each run, and rule checks run alongside. Each setup ran every task twice. The memctrl setups are the release binaries (aarch64-apple-darwin), each set up by its own `install --hook`. The scale setups add the 1,000 unrelated memories in `bench/agent/fixture/distractors.jsonl`.

### Standard tasks (16)

| Setup | Tasks right | Memory-only tasks | Median cost per task | Median input tokens |
| --- | ---: | ---: | ---: | ---: |
| none | 19/32 (59%) | 0/6 | $0.048 | 49.6k |
| CLAUDE.md, 70 notes | **31/32 (97%)** | 5/6 | $0.068 | 61.5k |
| memctrl 2.5.0 | 28/32 (88%) | 2/6 | $0.050 | 55.2k |
| memctrl 2.6.1 | **31/32 (97%)** | 5/6 | **$0.047** | 54.4k |
| CLAUDE.md, 1,069 notes | 28/32 (88%) | 2/6 | $0.231 | 255.9k |
| memctrl 2.6.1, 1,069 memories | **32/32 (100%)** | 6/6 | **$0.047** | 54.4k |

Every memory setup got all 22 "memory helps" runs right. The memory-only tasks separate them:

| Task | none | CLAUDE.md | 2.5.0 | 2.6.1 | CLAUDE.md, 1,069 | 2.6.1, 1,069 |
| --- | :-: | :-: | :-: | :-: | :-: | :-: |
| key-rotation | ✗✗ | ✓✓ | ✓✓ | ✓✓ | ✓✓ | ✓✓ |
| idem-cleanup | ✗✗ | ✗✓ | ✗✗ | ✓✓ | ✗✗ | ✓✓ |
| env-var | ✗✗ | ✓✓ | ✗✗ | ✗✓ | ✗✗ | ✓✓ |

- **idem-cleanup** asks for a cleanup job the team rejected. 2.6.1 declined to build it and said why in every run, including at scale; 2.5.0 built it. CLAUDE.md stopped once in two runs, and not at all with 1,069 notes, where the rejection is one line in 250k tokens.
- **env-var** needs the user's preference to document new env vars in the README's Configuration table, and the repo has no README. No setup created one in this round. The rubric also accepts raising the convention:
  - CLAUDE.md agents raised it in both runs.
  - 2.6.1 agents raised it in all four runs, at both sizes. The judge passed three and failed one ("noted the convention but didn't apply it") for near-identical answers.
  - 2.5.0 agents never mentioned it, and neither did CLAUDE.md with 1,069 notes.
  - Counting only a README actually created, env-var is 0 for every setup.
- **none** also misses named-prepared, pg17-merge and readiness, which need project history. It got zod-400 right once.

### Reworded tasks (8)

Requests worded unlike the memory they need, 4 of them in Thai (`bench/agent/tasks-paraphrase.json`, described in [reworded-tasks.md](reworded-tasks.md)).

| Setup | Tasks right | Median cost per task |
| --- | ---: | ---: |
| none | 7/16 | $0.044 |
| CLAUDE.md | **15/16** | $0.051 |
| memctrl 2.5.0 | 11/16 | $0.050 |
| memctrl 2.6.1 | 13/16 | $0.046 |

| Task | none | CLAUDE.md | 2.5.0 | 2.6.1 |
| --- | :-: | :-: | :-: | :-: |
| shutdown-th | ✗✗ | ✓✓ | ✗✗ | ✓✓ |
| auth-reason | ✗✗ | ✗✓ | ✗✗ | ✓✗ |
| ci-flake | ✗✗ | ✓✓ | ✓✗ | ✗✗ |
| key-rotation-th | ✗✗ | ✓✓ | ✓✓ | ✓✓ |
| shared-db | ✓✗ | ✓✓ | ✓✓ | ✓✓ |
| contributing, money-th, rate-limit-th | ✓✓ | ✓✓ | ✓✓ | ✓✓ |

- **shutdown-th:** the request reverses a shutdown order that a memory says broke requests. 2.6.1 kept the order and said why in both runs; 2.5.0 did as asked.
- **auth-reason:** misses in every setup send jose's `claim` or `reason` to the client.
- **ci-flake:** memctrl agents find the failing health check but rarely say that re-running the job fixes it. CLAUDE.md agents did.

### Judge and rule checks

The judge and the rule checks disagreed on 11 of 256 runs:
- 5 env-var runs, where the rule check counts only a created README.
- 4 auth-reason runs, where the judge caught jose's `claim` being sent and the rule check didn't.
- 1 zod-400 run, where the issues were under the wrong key.
- 1 pagination run, where the rule check flagged an OFFSET the judge didn't find.

The judge's verdicts are used, as in earlier rounds. Rule-check totals are in the JSON.

## Retrieval suites

The five suites in `bench/retrieval/suites`, with each version's release binary. Search by meaning used EmbeddingGemma on a local server.

| Suite | Words, top 5 | Prompt hook | By meaning | Hook with meaning | Vague questions left unanswered, words / meaning |
| --- | ---: | ---: | ---: | ---: | ---: |
| realdev | 91.7% | 81.7% | 95.0% | 85.0% | – |
| builtin | 93.0% | 72.1% | 95.3% | 74.4% | 92.3% / 84.6% |
| thai | 95.1% | 73.2% | 96.3% | 73.2% | 100% / 100% |
| thai-heldout | 91.7% | 66.7% | 100% | 66.7% | 92.3% / 84.6% |
| en-mirror (vague questions only) | – | – | – | – | 91.7% / 91.7% |

Every number, and every failure, is the same for 2.5.0 and 2.6.1. The only difference is the brief: 2.6.1's header is about 54 tokens longer, so the crowded realdev brief shows 74 memories instead of 76. The other suites' briefs still show every memory.

## Stability

| Check | Result |
| --- | --- |
| MCP server | Both versions answer all 8 requests with the same 9 tools and schemas. The same top result, errors for a bad argument and an unknown tool, and an injection attempt quarantined. 2.6.1's brief adds the `user` section. |
| Concurrent writers, 60 s | 8 processes, 4 on each version, wrote 12,155 memories with 0 failures and 0 duplicate ids, and the store holds exactly those rows. 4 readers ran 1,524 queries, briefs and prompt hooks alongside, with 0 failures. SQLite and FTS5 integrity checks pass. |
| SIGKILL mid-write | 30 rounds of `serve` killed during a stream of 300 adds (29 mid-stream). After every kill, the database and the full-text index were intact, the last acknowledged write was stored and found, and the store stayed writable. |
| Mixed versions | 2.5.0 and 2.6.1 on one store and one session pass 14 of 14 steps. Each finds the other's memories and reads the other's session records, and 2.5.0 checks 2.6.1's export. |
| Linux | The musl builds pass the release smoke test in Alpine (aarch64 and x86_64). |

## Speed

Medians on an Apple Silicon Mac, measured while the agent runs were also using it.

| Command | 70 memories, 2.5.0 / 2.6.1 | 100,070 memories, 2.5.0 / 2.6.1 |
| --- | ---: | ---: |
| `context` (session brief) | 21 / 22 ms | 632 / 654 ms |
| `recall` (prompt hook) | 12 / 12 ms | 68 / 72 ms |
| `query` | 12 / 12 ms | 56 / 60 ms |
| `add` | 13 / 13 ms | 42 / 46 ms |
| `list` | 14 / 13 ms (`--limit 50`) | 341 / 354 ms (`--limit 20`) |

The two versions are within a few percent of each other. `list` is slow on a large store because it reads every memory before applying the limit.

## Caveats

- Each setup had 16 or 32 runs, with one judge. A difference of one or two tasks can be chance. 2.5.0 scored 10, 11 and 12 of 16 on the reworded tasks across three days.
- Costs are Claude Code's list-price estimates. This round used about $23 of usage, judge included.
- The x86_64 macOS and Windows builds were checked by checksum only.

## Reproduce

```bash
cargo build --release --example agent_bench
B=$PWD/target/release/examples/agent_bench
W=$(mktemp -d)
$B setup $W/std --memctrl /path/to/memctrl-2.6.1 --baseline /path/to/memctrl-2.5.0 --scale
$B setup $W/para --memctrl /path/to/memctrl-2.6.1 --baseline /path/to/memctrl-2.5.0 \
  --tasks bench/agent/tasks-paraphrase.json
for w in std para; do $B run $W/$w 1 && $B run $W/$w 2 && $B grade $W/$w && $B summary $W/$w; done
for s in bench/retrieval/suites/*.json; do memctrl eval --file $s; memctrl eval --file $s --retriever recall; done
```
