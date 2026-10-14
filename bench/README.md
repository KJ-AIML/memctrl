# Benchmarks

Two benchmarks, re-runnable before each release. Results and write-ups are in `docs/benchmarks/`.

| Folder | Question | Cost |
| --- | --- | --- |
| `agent/` | Does Claude Code do real tasks right more often with memory? | Claude Code usage: about $0.06 a run, $0.23 a run for CLAUDE.md at scale |
| `retrieval/` | Does memctrl find the right memory, and say nothing when it should, compared with other tools? | Local only |

## Agent benchmark

You need Claude Code (`claude`) logged in, Python 3, git, and Node 23.6 or later, which the grader uses to run TypeScript directly. The runs use your Claude Code account.

```bash
cargo build --release
W=$(mktemp -d)                                # anywhere outside the repo
python3 bench/agent/setup.py $W --memctrl target/release/memctrl \
  --baseline /path/to/previous/memctrl --scale 1000   # both flags optional
python3 bench/agent/run.py $W 1 --jobs 4      # one round: every task in every setup
python3 bench/agent/run.py $W 2 --jobs 4      # a second round
python3 bench/agent/grade.py $W               # rule checks plus a blind Sonnet judge
python3 bench/agent/summary.py $W
```

- `setup.py` builds one template per setup from `fixture/`, which holds the ledger-api project as a git bundle, the store its tester saved (`memories.sqlite3`) and its `.memoryrc`.
- Each run copies a template and starts `claude -p` with only that template's settings: no user settings, plugins, MCP servers or auto memory.
- `--only setup/task` reruns single cells. A run interrupted halfway is set aside, not deleted.
- `recall_probe.py BIN` shows what the prompt hook adds for every task and for 15 unrelated everyday prompts. Run it inside a copy of a memctrl template, because queries in the template itself change what later runs see.

To add a task, add an entry to `tasks.json` with a prompt and a rubric the judge can apply from the final message and the diff. Then add a rule check to `grade.py`.

## Retrieval comparison

```bash
python3 bench/retrieval/bm_run.py $OUT $TOOLS     # optional: basic-memory, from a venv in $TOOLS/bm-venv
python3 bench/retrieval/compare.py $OUT $MODELS target/release/memctrl /path/to/previous/memctrl
python3 bench/retrieval/startup.py target/release/memctrl new   # run inside a project
```

- `$MODELS` holds ONNX exports with their `tokenizer.json`: `e5-small/onnx/model_quantized.onnx` (multilingual-e5-small) and `gemma/onnx/model_q4.onnx` (EmbeddingGemma 300M).
- `compare.py` needs `numpy`, `onnxruntime` and `tokenizers`.
- The suites are in `retrieval/suites/`.
