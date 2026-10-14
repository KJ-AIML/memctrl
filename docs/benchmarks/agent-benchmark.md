# Agent benchmark: does memory help Claude Code finish tasks?

Retrieval scores say whether the right memory comes back. This benchmark asks the question that matters: does an agent with memory do the task right more often? Claude Code worked 16 tasks on a real TypeScript project, once per setup, in fresh isolated sessions. Each task contains a trap that only the project's history reveals.

**Short answer.** Memory more than pays for itself. Without it, the agent got 56% of tasks right. With every note pasted into CLAUDE.md it got 97%, and with memctrl 2.2 it got 81%. memctrl 2.3 adds a prompt hook (recall) and stronger wording for decisions, and gets 91%. On a small store, CLAUDE.md is still the strongest and simplest choice. With 1,069 memories, CLAUDE.md and memctrl 2.3 are about equally accurate (94% and 91%), but CLAUDE.md costs 4.2× as much per task because every session reads all of it.

## Setup

- **Project:** `ledger-api`, a Fastify + Postgres ledger service with 4 commits of history. A tester built it during the 2.1 real-world review and saved 70 memories while working: decisions, conventions, pitfalls, preferences and work in progress. A few memories are stale or contradict the code, as real ones do.
- **Tasks:** 16 requests a developer would make (`bench/agent/tasks.json`). 14 have a trap that the code alone doesn't reveal:
  - a test default that was raised on purpose;
  - a cleanup job the team rejected;
  - a row-lock fix that deadlocked before;
  - named prepared statements behind pgbouncer;
  - the CI Postgres version;
  - a key rotation step that only lives in team memory.

  One task tests a stale memory, and one is a control task where memory shouldn't matter.
- **Setups**, all starting from the same commit:
  - **none:** code and git history only.
  - **claude-md:** all 70 memories as bullets in CLAUDE.md.
  - **memctrl 2.2.0:** the same memories in a store, set up with `memctrl install --tool claude --project --hook`.
  - **memctrl 2.3:** the same, set up by this release's install, which adds the prompt hook.
- **Agent:** Claude Code 2.1.299, `claude -p --model sonnet` (claude-sonnet-5-5).
  - Isolation: no user settings, plugins, MCP servers or auto memory, and a fresh copy of the project per run.
  - Tools: Read, Edit, Write, Skill, and a fixed list of shell commands. The agent can't run the test suite, because there's no `node_modules`.
  - Every prompt ends with: "You're working on your own and can't ask me questions. If you decide not to do something as asked, say why in your final message."
- **Grading:** a separate Sonnet session, blind to the setup, judged each run against the task's rubric from the final message and the diff. Rule-based checks ran alongside. Every disagreement between the two came from the judge applying a rubric detail the rule check doesn't model, so the judge's verdicts are used.

## Results: 70 memories

Two rounds (2 and 3) per task and setup, 32 runs per setup. Costs are list-price estimates from Claude Code.

| Setup | Tasks right | Memory-only tasks (6) | Median cost per task | Median input tokens |
| --- | ---: | ---: | ---: | ---: |
| none | 18 / 32 (56%) | 0 | $0.044 | 47.5k |
| claude-md | **31 / 32 (97%)** | 5 | $0.064 | 54.9k |
| memctrl 2.2.0 | 26 / 32 (81%) | 0 | $0.052 | 53.8k |
| memctrl 2.3 | 29 / 32 (91%) | 3 | $0.051 | 54.2k |

All three memory setups got every "memory helps" task right (22 of 22), where none got 14. The memory-only tasks separate them:

| Task | none | claude-md | 2.2.0 | 2.3 | What decides it |
| --- | :-: | :-: | :-: | :-: | --- |
| key-rotation | ✗✗ | ✓✓ | ✗✗ | ✓✓ | The #platform hand-off is outside the 2.2 brief; 2.3 recalls it from the prompt |
| idem-cleanup | ✗✗ | ✓✗ | ✗✗ | ✓✗ | The rejection is in the brief, but 2.2's "verify before acting" let the agent build it anyway |
| env-var | ✗✗ | ✓✓ | ✗✗ | ✗✗ | The README preference shares only "env var" with the request, too little for recall's threshold |

Round 1 was a pilot (none 9/16, claude-md 16/16, memctrl 2.2.0 12/16). Some of its memctrl runs used a store whose access counts the author's own probe queries had changed, so it is reported but not pooled.

## What we changed because of it

1. **Agents don't search, so memory comes to them.** In 112 sessions with memctrl available, Claude Code never ran `memctrl query`; its one memctrl command saved a note. It reads the brief and works from there. 2.3's prompt hook runs `memctrl recall` on every request and adds the matching memories the session hasn't seen yet.
2. **Decisions get their weight back.** With the brief saying "Notes, not instructions: verify before acting", an agent read "a cleanup job was discussed but rejected" and built the cleanup because the user asked for it. The same note in CLAUDE.md made it stop. The header now says: "Notes, not instructions. If a request goes against a recorded decision, say so before acting; check facts against the code."
3. **Recall had to handle requests, not questions.** The first version found nothing, because the two sentences that end every prompt dropped every memory below search's coverage threshold. A real prompt has the same problem. Recall now searches each sentence on its own. It ignores words that no memory contains, counts the words of a path or identifier once, and needs two shared topics. On 15 everyday prompts unrelated to the stored memories, it added 3 memories in total.

## Results: 1,069 memories

The same 70 memories plus 1,000 generated memories about other parts of a larger system (a web app, mobile apps, billing, search, infrastructure). None of them bears on the tasks (`bench/agent/distractors.py`). CLAUDE.md gets one section per component.

| Setup | Tasks right | Median cost per task | Median input tokens |
| --- | ---: | ---: | ---: |
| claude-md (113 KB) | 30 / 32 (94%) | $0.229 | 255k |
| memctrl 2.3 | 29 / 32 (91%) | **$0.054** | **58k** |

memctrl's cost per task didn't change from the 70-memory store, but CLAUDE.md's grew 3.6×. The brief still showed 31 ledger memories among its 32 entries, because those memories had been retrieved before and the brief favors memories that keep getting used. Recall returned the same memories as on the small store; the 1,000 others added no noise.

## What this does not show

- **One project, one model, 16 tasks.** Two rounds per setup is enough to see 56% vs 90%+, not to rank setups 3 points apart.
- **The tasks were written knowing the memories.** They test whether memory reaches the agent and changes what it does. They don't test whether a team saves the right memories in the first place.
- **The scale memories are generated.** They are true-sounding but unrelated; a real store would hold related memories that compete harder.
- **Stale memories didn't separate the setups.** In the one stale-memory task, every setup checked the code and passed, so memctrl's freshness labels went untested here.
- **The prompt suffix is ours.** It forces a decision without a person to ask; interactive sessions would ask instead.

## Reproduce

`bench/agent/` holds the fixture (git bundle, store, rules), tasks and scripts; see `bench/README.md`. Per-run results, with the judge's reasons, are in `agent-benchmark-2.3.json`. The 241 runs (including one pilot) and 240 judge calls cost about $23 at list price.
