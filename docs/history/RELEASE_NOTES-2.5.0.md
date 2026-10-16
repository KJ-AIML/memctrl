# MemCtrl 2.5.0: memory for requests in other words

People don't always ask in the words a memory uses, and agents don't search on their own. In 2.4, a memory the session brief left out reached the agent only if the request shared its words. 2.5 shows more of the store up front, and the prompt hook points out the brief memories a request needs.

## Highlights

- **A small store arrives whole.** The session brief holds up to 3,000 tokens (1,500 before): about 70 one-line memories, the way a CLAUDE.md would.
- **The prompt hook points out what a request needs.** It used to skip every memory the brief had shown, so a bigger brief silenced it. Now a brief memory that matches the request is pointed out once, next to the request, marked "also in the session brief". It still never adds a memory twice in a session.
- **Measured against 2.4.0 in the same rounds:**
  - On 8 tasks worded unlike the memory they need, 4 of them in Thai, 2.5 got 12 of 16 right; 2.4.0 got 8.
  - On the 4 of those tasks that can't be done from the code alone, it was 4 of 8 runs, against 0 of 8.
  - On the 16 standard tasks, both got 28 of 32.
  - The median cost per task came out a little lower. [Full results](../benchmarks/reworded-tasks.md).
- **Benchmark harness:**
  - `agent_bench setup --tasks FILE` runs another task set; the reworded tasks are in `bench/agent/tasks-paraphrase.json`.
  - `probe` says whether each task's memory reaches the agent, and marks the brief memories the hook points out again.

## What it doesn't change

- **Search by meaning doesn't reach these requests either.** EmbeddingGemma often ranks the right memory first, but at a similarity of 0.34 to 0.49, while unrelated everyday prompts reach 0.54. No threshold separates the two, so the prompt hook added none of these memories with or without meaning.
- **Being in the brief isn't the same as being followed.**
  - With the README preference in the brief, agents still skipped it in 3 of 3 runs; from CLAUDE.md they follow it.
  - Half the agents still did what was asked against a recorded reason.
  - Put rules you want followed in CLAUDE.md.

## Upgrade notes

See the 2.4 → 2.5 section of [MIGRATION.md](../../MIGRATION.md). Hooks set up by `install --hook` pick up the new brief size; add `--budget 1500` to the SessionStart command to keep the old one. Session records from 2.4 keep working.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](.).
