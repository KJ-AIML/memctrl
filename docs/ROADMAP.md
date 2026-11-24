# memctrl roadmap

Written 2026-11-24, after a review of 2.8.0. It replaces feature-by-feature planning with one question per milestone: does this make memctrl something its own author reaches for every day?

## Where 2.8.0 stands

memctrl delivers memory well and has the numbers to show it. What it doesn't have is a way for memory to get in, or anything that keeps work going across `/clear`, compaction and worktrees. The evidence:

- **Delivery works.** With 70 notes, memctrl 2.6.1 and a CLAUDE.md both got 31 of 32 benchmark tasks right; memctrl cost less ([release-2.6.md](benchmarks/release-2.6.md)).
- **Capture is manual.** In 112 benchmark sessions, agents ran one memctrl command. A store fills only when a person types `memctrl add` or starts a prompt with `remember:`.
- **The quickest capture path loses the note.** `memctrl add "…"` without `--layer` writes to `session`, which the brief and recall never read (`context::DEFAULT_LAYERS` is `project`, `user`) and which expires in seven days. The installed skill tells agents to save work in progress there.
- **A linked worktree has no memory.** `project::locate` stops at the folder that holds `.git` and uses its `.memctrl/memories.db`, which git ignores, so every `git worktree add` starts empty.
- **Nothing is saved when context is lost.** The installer sets up `SessionStart` and `UserPromptSubmit` only. No code reads `Stop`, `PreCompact` or `PostCompact`, so the goal, the next step and the dead ends of a long session die with it.
- **Sub-agents get nothing.** No `SubagentStart` hook, and recall is keyed on the words of a prompt a sub-agent never receives from a person.
- **Citations work one way.** A citation flags a stale memory; it is never used to find the memories about the file an agent is touching. On eight tasks worded unlike their memories, recall added none of the needed ones ([reworded-tasks.md](benchmarks/reworded-tasks.md)).
- **The benchmark's baseline is not the real competitor.** Runs disable Claude Code's auto memory, and the 1,069-note comparison is against a CLAUDE.md five times the size its own docs recommend.
- **About a fifth of the code is outside the daily loop:** `tree`, `decay`, `reflection`, OTel spans and export, agent checkpoints from the LangGraph port, the Heli importer, four LLM providers, and triggers in `.memoryrc` that no hook fires. The CLI has 47 top-level commands; the loop uses about six.

## Position

memctrl stops competing with CLAUDE.md and AGENTS.md for rules. A short rules file wins there, and the README already says so. memctrl owns what a file can't do:

1. **Work that survives.** `/clear`, compaction and a new worktree pick up where the branch left off.
2. **Memory that arrives by file.** Touch `src/db/ledger.ts` and the pitfalls recorded about it are there, whatever the prompt said, in sub-agents too.
3. **Memory that knows when it's stale.** Cited lines changed, so the note says "check before trusting".
4. **One store for every agent.** Claude Code, Codex and Gemini CLI read and write the same project memory.

## Rules for deciding

- **Nothing the user must remember to do.** A feature that needs a typed command during work is not a capture feature.
- **Hooks' documented fields are the only inputs.** No transcript parsing: Codex's docs say the transcript format "isn't a stable interface for hooks", and Claude Code's isn't documented either.
- **No model in the default path.** Anything that needs one is opt-in from the user's own config, like `[semantic]` today.
- **A hook never fails a session** and never creates a store where memctrl wasn't set up.
- **Measure on the author first.** A milestone is done when it changes two weeks of real use, then when a benchmark agrees.
- **Add nothing to retrieval tuning, semantic search, guard patterns or new hosts** until milestone 3 has shipped.

## Milestones

### M0. Use it (no code)

- Install the hooks once for the user: `memctrl install --tool claude --hook` (and `codex`, `gemini`).
- `memctrl init` in the two or three repositories worked in most.
- In this repository: stop ignoring `/.memctrl/` wholesale in `.gitignore` so `memory.md` can be committed; the folder's own `.gitignore` already keeps the database out.
- Write down the baseline: memories per store, how many were typed by hand, how many sessions got a non-empty brief.

Exit: memctrl's hooks run in every session on the author's machine.

### M1. Continuity (2.9.0)

Work on a branch survives `/clear`, compaction, a new session and a new worktree, with nothing typed.

- A checkpoint per branch, written by hooks from fields the hosts already hand over: the user's last requests, the agent's last message, the compaction summary, the files changed.
- Shown when a session starts fresh or after `/clear`, as notes above the brief.
- Linked worktrees share the main worktree's store.
- Session-layer notes show up in the checkpoint, so the skill's advice finally works.
- `memctrl add` defaults to the `project` layer.
- The brief stays under Claude Code's 10,000-character cap on hook output, past which the model gets a 2,000-character preview and a file path.

Design: [superpowers/specs/2026-11-24-continuity-design.md](superpowers/specs/2026-11-24-continuity-design.md). Plan: [superpowers/plans/2026-11-24-continuity.md](superpowers/plans/2026-11-24-continuity.md).

Exit: for two weeks the author runs `/clear` freely and the next session continues without the request being retyped; a benchmark task that forces a compaction mid-way passes at least as often as an uncompacted run.

### M2. Memory by file (2.10.0)

- **Two kinds of link.** "About this path" is a recall key and never goes stale; "asserts these lines" is today's citation. A citation with no line range compares the whole file's hash (`citations::check`), so auto-citing at file level would mark every memory stale on the next edit.
- **Recall by path.** Memories linked to a file arrive when an agent reads or edits it: `PreToolUse` on Claude Code and Codex (`tool_input.file_path`, `additionalContext`), `AfterTool` on Gemini CLI. Tool hooks run inside sub-agents, so delegates get them too. Once per file per session.
- **Recall by working tree.** The prompt hook also considers files with uncommitted changes, for prompts like "tests are still red".
- **Staleness in the loop.** When an edit changes cited lines, the agent that made the edit is told which memory to update or refute, in place of a `memctrl stale` queue.
- **A brief for sub-agents.** `SubagentStart` adds pins, the user's preferences and the branch checkpoint's notes, in about 600 tokens.
- **Links without typing.** `memctrl add` with no `--cite` links the memory to the files changed in the working tree.

Exit: the reworded-task suite reaches CLAUDE.md's score (15 of 16; 2.6.1 got 13), and a task delegated to a sub-agent that needs a file's pitfall passes.

### M3. Capture (2.11.0)

- **A session journal.** M1 keeps the last exchange; M3 keeps a bounded list of request and reply pairs per session, local and redacted.
- **Proposed memories.** When a session ends or compacts, an opt-in model pass over the journal proposes candidates: corrections the user made, decisions with reasons, approaches that failed, commands that finally worked. Each is linked to the files of its turn.
- **Ten-second review.** `memctrl review` walks proposals one key each. Notes an agent proposed join the store at low authority and expire unless recalled; a person's key press is needed only to make one a decision or the user's own word. What the guard holds still needs the typed id.
- **Visible value.** The session-start line says what was picked up; a weekly digest lists which memories reached a session and which stopped an agent.
- **One place for the user's own preferences**, shared by every project.

Exit: a store gains at least five accepted memories a week from normal work, with no `memctrl add` typed.

### M4. Smaller (3.0.0)

Remove what the loop doesn't use: `tree`, `decay`, `reinforce`, `reflect`, `done`, `trigger` and the `[triggers]` table, `heatmap`, `timeline`, `spans`, `otel-export`, `otel-stats`, `agent`, `review heli`, the legacy keyword scorer outside `eval`, and all but one LLM adapter. Each is deprecated with a warning in 2.11 and deleted in 3.0, with the schema's tables left in place so old stores open. Also delete `memctrl/templates/SKILL.md`, a 1.x file nothing installs.

Exit: under 25 commands, and `make ci` time down by the share of tests removed.

## Evidence, alongside the milestones

| | What | Why |
| --- | --- | --- |
| E1 | Two weeks of the author's own use after M0 and after each milestone | The benchmark never tested whether memory gets saved |
| E2 | Agent benchmark arm: auto memory on, with a 150-line CLAUDE.md | The real alternative; if it ties, the position above is all that's left to sell |
| E3 | Benchmark tasks that force `/compact` or `/clear` mid-task, and one delegated to a sub-agent | M1 and M2 claim these; nothing measures them today |
| E4 | One session with a brief over 10,000 characters, checking what the model received | Confirms the cap M1 works around |

## Risks

- **Checkpoints replay agent text into the next session.** The guard checks each part and the block is framed as notes; a part it flags is left out. A paraphrased injection can still pass, as with memories.
- **The checkpoint keeps the user's last three requests** in a local file, where 2.8 stored no prompt at all. It is redacted, ignored by git and can be turned off.
- **Hosts differ.** Only Claude Code hands over a compaction summary; Codex and Gemini CLI get the last exchange and the files.
- **Hook contracts change.** Every field M1 reads was checked against the hosts' docs on 2026-11-24; the installer and tests name each one so a change fails loudly.
