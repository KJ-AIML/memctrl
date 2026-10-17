# MemCtrl 2.6.2: listing a large store without reading all of it

A speed fix found by the 2.6.1 benchmark. `memctrl list` and `memctrl candidates` loaded every memory in the store, then kept the first `--limit`. Now they stop reading once they have enough.

## Fixes

- **`list` and `candidates` read only what they show.** On a store of 100,000 memories:

  | Command | 2.6.1 | 2.6.2 |
  | --- | ---: | ---: |
  | `list --limit 20` | ~800 ms | ~240 ms |
  | `candidates` | ~820 ms | ~75 ms |

  On small stores both stay around 13 ms. The output is the same as before: the newest first, ties by id, with the same eligibility, `--layer` and `--include-history` rules. A test compares the two for every combination.

## 2.6 in short

- **Your preferences come first.** User-layer memories you saved are shown under "From the user", and the brief says to follow them unless the request says otherwise.
- **Recorded decisions wait for you.** If what's asked can't be done without going against a recorded decision, the agent names the memory and asks before changing anything.
- **Measured on the 2.6.1 release:** 31 of 32 standard tasks right, the same as CLAUDE.md at a lower cost per task. With 1,069 memories, 32 of 32 at a fifth of CLAUDE.md's cost. [2.6.0 notes](RELEASE_NOTES-2.6.0.md), [2.6.1 notes](RELEASE_NOTES-2.6.1.md).

## Upgrade notes

See the 2.6.1 → 2.6.2 section of [MIGRATION.md](../../MIGRATION.md). Nothing in the database, the brief or the output changes.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](.).
