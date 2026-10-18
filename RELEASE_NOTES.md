# MemCtrl 2.8.0: Thai and English, under stress

A stress test of memctrl on a real project, whose team writes notes in Thai and English and asks in both, found eight problems. Questions in one language didn't find notes in the other, a scrap of Thai outranked the name of a command, the guard missed instructions written in Thai, and a store where every note had the same trust got a brief of the newest ones. This release fixes all eight. Each fix has a test, and the retrieval fixes have a new public suite with floors in CI.

## What's new

- **Thai and English find each other.** The built-in aliases now hold the Thai words developers use for 37 technical terms, loanwords and plain Thai alike: ดีพลอย and deploy, ฐานข้อมูล and database, โปรดักชัน and production, ลบ and delete, and so on. They're picked out of Thai text written without spaces, so `ดีพลอยขึ้นโปรดักชันต้องขออนุมัติใคร` finds "Deploys to production need an approval from the on-call lead", and `delete production database` finds `ห้ามลบฐานข้อมูลบนโปรดักชัน…`. Your own aliases work inside Thai text the same way. memctrl has no Thai dictionary, so it reads a run by the words it knows: a two-letter word such as ลบ only counts where a word is known to end, so ข้อมูลบนเซิร์ฟเวอร์ ("data on the server") doesn't search for delete.
- **A named identifier counts.** `reset-ledger` or `src/db/ledger.ts` in a question is one more thing it names, covered only by a memory that holds it whole (`reset-ledger (identifier)` in `why`). In a Thai question that names a command, the memory about the command now comes first, and the prompt hook counts a whole name joined with `-` or `_` as a second topic; file names and paths still count once.
- **Pin what matters.** `memctrl pin <id>` puts a memory first in every session brief, in up to half its budget, marked `pinned`. Only a person pins: `pin` and `unpin` need a terminal, like releasing a held memory, and both go to the audit log.
- **A decision an agent saved is a note until you confirm it.** The brief tells agents to stop and ask before going against a recorded decision, and any agent could record one. Now a decision from a source the guard doesn't trust is shown among the facts, marked "decision saved by an agent, not confirmed" (`[note]` in recall), until you confirm it with `memctrl accept <id>` in your terminal. Your own decisions count from the start.
- **The guard reads Thai, and more.** It holds Thai and mixed Thai and English requests to ignore the agent's instructions (ไม่ต้องสนใจคำสั่งก่อนหน้า, ignore คำสั่งก่อนหน้าทั้งหมด, จากนี้ไปคุณคือ…), the same request in Chinese, Japanese, Spanish, French and German, agent harness tags such as `<system-reminder>`, `SYSTEM:` lines, and text imitating memctrl's own `[from the user]` label. On the red-team corpus it catches 37 of 73 attempts (26 in 2.7.0) and still holds 4 of 50 ordinary notes; a new Thai set of 12 attempts and 17 near-miss notes is judged right in every case.

## Fixes

- **A fragment inside a word no longer matches.** The substring fallback counted `lag` inside "flagged"; now it counts only where a word starts, camelCase humps included (`connections` still finds `maxConnections`). A memory found only that way isn't counted as used, so it can't climb in the brief.
- **Vague Thai requests add nothing.** The prompt hook treats Thai words for what to do (ทำ, ต่อ, ดู, แก้ …) as generic and drops particles such as เลย and ด้วย, so ทำต่อเลย ("keep going") no longer brings in notes that happen to contain those words.
- **Run ids are no longer phone numbers.** Every run of 10 to 12 digits was redacted as a phone number, CI run ids and order numbers included. Now such a number is redacted when it starts with 0 or a phone word comes just before it (โทร, เบอร์, phone, call …), never right after `run`, `order`, `ref` or `#`. Numbers with `+` and the usual US formats are redacted as before.
- **Zero-width spaces between a Thai and an English word** (ห้าม​deploy​วันศุกร์) no longer hold a note for review.
- **Reading no longer writes.** Every command recorded a timing span, so the hooks wrote to the store on every session and prompt, and `stats` or `timeline` grew it. Now only commands that change the store record one.
- **The brief says how it chose.** When something doesn't fit, the header says the newer memory wins at equal trust.
- **Recall in a small store** breaks ties by coverage; scores that round to zero used to leave the order to the memory ids.

## Numbers

Public suites in the repository, `memctrl eval`, 2.7.0 → 2.8.0:

| Suite | Answer in the top 5 | Answer first | Empty questions answered with nothing | Prompt hook adds the answer |
| --- | ---: | ---: | ---: | ---: |
| Thai team (independent reviewer) | 95.1% → 97.6% | 92.7% → 95.1% | 100% → 100% | 73.2% → 74.4% |
| Thai held out (never used for tuning) | 91.7% → 91.7% | 83.3% → 91.7% | 92.3% → 92.3% | 66.7% → 91.7% |
| Bilingual (new, written by memctrl's author) | 52.9% → 100% | 52.9% → 94.1% | 66.7% → 100% | 29.4% → 82.4% |
| Real project, built-in, English mirror | unchanged | unchanged | unchanged | unchanged |

## Upgrade notes

See the 2.7.0 → 2.8.0 section of [MIGRATION.md](MIGRATION.md). In short:

- **Rankings change** for Thai and mixed questions, identifiers and word fragments. Cached results from 2.7 are ignored.
- **Decisions your agents saved become notes** until you confirm them. The brief's header counts them; confirm each with `memctrl accept <id>` in a terminal.
- **Two columns are added** to `memories` (`pinned_at`, `confirmed_at`). Older 2.x versions ignore them.
- Run `memctrl install` again with your `--tool`, so the skill tells your agent how decisions get confirmed.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](docs/history).
