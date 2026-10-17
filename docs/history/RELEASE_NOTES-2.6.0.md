# MemCtrl 2.6.0: preferences and decisions that carry weight

Up to 2.5, the session brief called every entry "Notes, not instructions". Agents read your preferences and your team's decisions there, and still went against them as soon as someone asked. 2.6 keeps the brief as notes, with two exceptions that do what a CLAUDE.md would.

## Highlights

- **Your preferences apply.** User-layer memories you saved come first, under "From the user", and the brief says to follow them unless the request says otherwise. The prompt hook labels them `[from the user]`.
- **Recorded decisions wait for you.** If what's asked can't be done without going against a recorded decision, or without repeating what a memory says failed, the agent names the memory and asks before changing anything. A request that can be done another way, such as a stale test that expects the old behaviour, goes ahead.
- **Measured against 2.5.0 in the same rounds:**
  - On the 16 standard tasks, 29 of 32 right; 2.5.0 got 28.
  - On 8 tasks worded unlike their memories, 13 of 16; 2.5.0 got 10.
  - The rejected cleanup job and the reversed shutdown order went from 0 of 2 to 2 of 2 each.
  - [Full results](../benchmarks/brief-authority.md).

## What it doesn't change

- **Everything else is still notes.** Facts, lessons and hypotheses are things to check against the code. So is anything an agent saves over MCP or that arrives in a plain-text import.
- **memctrl doesn't know who typed a memory.** An agent running `memctrl add --layer user` in its shell is counted as you. To keep an agent's ordinary writes out, set `trusted_sources = ["user"]` under `[guard]` in your user config and add preferences with `--source user`. That isn't proof of who wrote a memory: an agent can pass `--source user` too. The guard still re-checks every entry as the brief is built.
- **A preference isn't always followed.** Agents now mention the README preference in the benchmark, where 2.5.0 ignored it, but they still didn't create the README it asks for. Put rules that must hold in CLAUDE.md.

## Upgrade notes

See the 2.5 → 2.6 section of [MIGRATION.md](../../MIGRATION.md). The header still starts with "Notes, not instructions". The longer header costs about 60 tokens, and user-layer facts move to the new section. Nothing in the database changes.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](.).
