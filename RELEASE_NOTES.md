# MemCtrl 2.2.0: memory you can trust, as a team

We ran memctrl 2.1.0 through a real-world review: five testers played real use cases end to end, and every serious finding was reproduced independently. The storage engine held up. What didn't was everything that decides what an agent is allowed to believe. 2.2 closes those gaps and makes team sharing work.

## Highlights

- **Only a person releases a held memory.** The agent that was tricked into writing a quarantined memory could accept it itself. Now MCP refuses, and `memctrl accept` works only at a terminal: it shows the text with hidden characters spelled out and asks you to type the id back.
- **Pull requests can't plant memories.** An entry in `memory.md` whose metadata looked valid imported as accepted, so one PR could put a "decision" in every teammate's brief. New entries now arrive as candidates until a person runs `memctrl sync --accept` (or `import --accept`) after reading the diff.
- **Corrections reach your teammates.** Refuting, superseding or forgetting a memory adds a record to the file's new Retracted section; teammates' stores apply it on sync and never revive it. Export merges instead of overwriting, so nobody's entries vanish. `memctrl sync` is the one command to run after pulling, and `memctrl check --base origin/main` validates the file in CI without a database.
- **A cloned repository can't weaken your defenses.** Your settings live in `~/.config/memctrl/config.toml`; a repository's `.memoryrc` may only tighten them, and a database committed to git is never loaded.
- **The brief keeps its warnings.** With many memories, "Check before trusting" used to disappear. It now has reserved room, and the header says what didn't fit and when memories wait for your review.
- **Citations stay honest.** `refresh` follows moved code, or asks for the range with `--cite` when it can't tell, instead of re-hashing the old lines.
- **Works from any folder.** Commands find the project's store and rules from any subfolder, and only `memctrl init` creates a store. The 8-character ids in the brief work everywhere.
- **Better secret redaction.** Prefixed keys (`DB_PASSWORD=`, `STRIPE_API_KEY=`), GitLab, Slack, Stripe, npm and Google tokens, JWTs, encrypted keys, cards and phone numbers used to be stored in plain text. On the review's 34-format corpus, 33 are now redacted (the one left is a malformed fake key), up from 12–14. The key name stays, and writes report what they removed.
- **Far fewer false alarms.** The guard now holds 4 of 50 ordinary notes instead of 36 (installers, emoji, Thai word breaks), and catches 26 of 73 attacks instead of 19.
- **Vague questions get an honest "nothing".** "Summarize the project" no longer returns every memory that mentions a project, in any language. On a reviewer's Thai team suite, vague questions answered with nothing went from 59% to 100% (held-out questions: 77% to 92%) with no loss on real questions. English glued to Thai or Chinese is found, and single Thai letters stop matching inside words.
- **Fast and small under load.** Reads no longer take the write lock: on a 100,000-memory store with another process writing, the session-start brief answers in under a second (2.1.0 failed after 30 s). The query log keeps ids instead of copies of memory text, with retention, and memories are capped at 64 KiB. `memctrl_tree` is read-only unless asked to rebuild.

## Upgrade notes

Read the 2.1 → 2.2 section of [MIGRATION.md](MIGRATION.md). The changes you'll notice:

- **Team:** run `memctrl sync` after pulling, and `sync --accept` in a terminal once you've read the diff. memctrl 2.1 can't read the new file format and its export drops retraction records, so upgrade together and add `memctrl check --base origin/main` to CI. The first export rewrites the file once in the new order; commit that on its own.
- **Settings:** if your `.memoryrc` sets `[guard] mode = "off"`, move that line to `~/.config/memctrl/config.toml`.
- **Stores:** commands no longer create a store wherever they run. Run `memctrl init` once per project; `memctrl doctor` lists stray stores that older versions created in subfolders.
- **Database:** the schema stays at version 4, so 2.0, 2.1 and 2.2 can share it. The first open strips memory text from old query logs; `memctrl maintenance --vacuum` reclaims the space.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](docs/history).
