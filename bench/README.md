# Benchmarks

Results and write-ups are in `docs/benchmarks/`. This repository is Rust-only, so the harness is a Cargo example.

| Folder | Question | Cost |
| --- | --- | --- |
| `agent/` | Does Claude Code do real tasks right more often with memory? | Claude Code usage: about $0.06 a run, and $0.23 a run for the CLAUDE.md setup at scale |
| `retrieval/` | Does memctrl find the right memory, and say nothing when it should? | Local only |

## Agent benchmark

You need Claude Code (`claude`) logged in, git, and Node 23.6 or later, which the grader uses to run TypeScript directly. The runs use your Claude Code account.

```bash
cargo build --release --example agent_bench
B=target/release/examples/agent_bench
W=$(mktemp -d)                                    # anywhere outside the repository
$B setup $W --memctrl target/release/memctrl \
  --baseline /path/to/previous/memctrl --scale \
  --semantic semantic.toml                        # all three flags optional
$B run $W 1 --jobs 4                              # one round: every task in every setup
$B run $W 2 --jobs 4                              # a second round
$B grade $W                                       # rule checks plus a blind Sonnet judge
$B summary $W
```

- **`setup`** builds one template per setup from `agent/fixture/`: the ledger-api project as a git bundle, the store its tester saved (`memories.sqlite3`) and its `.memoryrc`.
  - **memctrl** is set up by the build under test, and **baseline** by the release you name, each with its own `install --hook`.
  - **`--scale`** adds the 1,000 unrelated memories in `fixture/distractors.jsonl` to a CLAUDE.md setup and a memctrl setup.
  - **`--semantic FILE`** adds `memctrl-semantic` (and `memctrl-semantic-scale` with `--scale`): copies of the memctrl setups whose user config is FILE, a `[semantic]` section pointing at an embedding server that must be running for setup and runs. Setup embeds every memory first, the way `memctrl semantic index` would.
- **`run`** copies a template for each task and starts `claude -p` with only that template's settings: no user settings, plugins, MCP servers or auto memory. `--only setup/task` reruns single cells. A run interrupted halfway is set aside, not deleted.
- **`probe BIN`** shows what the prompt hook adds for each task and for 15 unrelated everyday prompts; matches by meaning are marked `≈` with their similarity. Run it inside a copy of a memctrl template, because queries in the template itself change what later runs see. Pass `--config-home WORK/xdg-memctrl-semantic` to probe a semantic setup.
- **`--tasks FILE`** on `setup` runs another task set; setup copies it to `WORK/tasks.json`, which `run`, `grade` and `summary` read. `agent/tasks-paraphrase.json` holds 8 tasks whose requests are worded unlike the memory they need, 4 of them in Thai, none of those memories in the session brief: they measure search by meaning. Each names the memory it needs (`needs`), and `probe --tasks FILE` says whether the agent gets it.

To add a task, add an entry to `agent/tasks.json` with a prompt and a rubric the judge can apply from the final message and the diff. Then add its rule check to `rules` in `examples/agent_bench.rs`.

## Retrieval suites

`retrieval/suites/` holds the five suites from the head-to-head comparison. They're in `memctrl eval` format, so memctrl's side of the comparison is one command per suite:

```bash
memctrl eval --file bench/retrieval/suites/thai.json
memctrl eval --file bench/retrieval/suites/thai.json --retriever semantic   # needs [semantic]
memctrl eval --file bench/retrieval/suites/thai.json --retriever recall     # what the prompt hook adds
```

The other side ran outside this repository, because those tools need Python: basic-memory 0.23.2, and vector search with multilingual-e5-small and EmbeddingGemma 300M (ONNX). `docs/benchmarks/retrieval-comparison.md` describes the method, and `retrieval-comparison-2.2.json` holds every system's scores.
