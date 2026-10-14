# MemCtrl 2.3.0: memory that reaches the agent

We measured whether memory actually helps. Claude Code worked 16 tasks on a real project, each with a trap only the project's history reveals, in fresh sessions with and without memory. The project had 70 memories. The agent got:

- 56% of tasks right with no memory;
- 81% with memctrl 2.2;
- 97% with every note pasted into CLAUDE.md.

The gap had two causes, and 2.3 fixes both. With 2.3 the agent gets 91% right. With 1,069 memories it matches a full CLAUDE.md (91% vs 94%) at a quarter of the cost per task. [Full results](docs/benchmarks/agent-benchmark.md).

## Highlights

- **Memories arrive with each request.** Across 112 benchmark sessions with memctrl available, the agent never searched memory. It read the session brief and worked from that, so anything outside the brief never reached it. A new prompt hook runs `memctrl recall` on every request and adds the matching memories the session hasn't seen yet. When nothing matches, it adds nothing. Re-run `memctrl install --tool claude --project --hook` to get it.
- **Recall reads requests, not just questions.** It searches a long prompt one sentence at a time and ignores words no memory contains. A memory must share two topics with the request; the words of one path or identifier count once, and verbs like *add* and *fix* don't count. On 15 everyday prompts unrelated to the stored memories, it added 2 memories in total, and query results on the eval suites are unchanged. A recall takes about 10 ms. It only reads, and it never stores your prompt.
- **Recorded decisions carry weight.** The brief used to say "Notes, not instructions: verify before acting". An agent read that the team had rejected a cleanup job and built it anyway, because the person asked. The same note in CLAUDE.md made it stop. The header now reads: "Notes, not instructions. If a request goes against a recorded decision, say so before acting; check facts against the code."
- **Faster commands.** The check that refuses a database committed to git now reads `.git/index` instead of starting `git` each time, unless the index can't answer. `list` and `query` dropped from 13.7 ms to about 10.6 ms and the brief from 19 ms to 16 ms, which undoes most of what 2.2 added.
- **Benchmarks you can re-run.** `cargo run --example agent_bench` runs the agent benchmark and the 1,069-memory scale test against any build. `bench/` holds the fixtures and the retrieval suites. `docs/benchmarks/` has the results, including a head-to-head comparison with basic-memory, vector search, grep and load-everything.

## Upgrade notes

See the 2.2 → 2.3 section of [MIGRATION.md](MIGRATION.md). The database doesn't change, so 2.0 through 2.3 can share it. Re-run the install command to add the prompt hook. Anything that parses the brief's header should match the prefix "Notes, not instructions".

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](docs/history).
