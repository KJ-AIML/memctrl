# AGENTS.md — MemCtrl

> This file is for AI coding agents. It assumes you know nothing about the project.

## Project Overview

**MemCtrl** (version 1.2.0) is a cognitive memory runtime for AI agents. It replaces passive vector-RAG with a hierarchical, explainable, and self-managing memory layer.

Key capabilities:
- **Hierarchical tree-based retrieval** (PageIndex-inspired) instead of black-box cosine similarity
- **Rule-governed memory layers**: `project` (forever), `session` (7 days), `user` (90 days)
- **Confidence decay**: inferred facts fade unless reinforced; explicit facts persist
- **Retrieval provenance**: full audit trail for every memory retrieved
- **OpenTelemetry export**: first reference implementation for `gen_ai.memory.*` semantic conventions
- **Security**: automatic secret/PII redaction before storage or LLM prompts
- **MCP server**: stdio transport for IDE integration (Claude Code, Cursor, Kimi Code, etc.)
- **LangGraph integration**: checkpoint saver and memory node

All data lives locally in SQLite by default. No cloud, no telemetry, no analytics.

## Technology Stack

- **Language**: Python 3.10+
- **Build system**: `hatchling` (PEP 517 backend)
- **Package manager**: `pip` or `uv` (recommended)
- **CLI framework**: `typer` + `rich`
- **Database**: SQLite (bundled with Python)
- **Config format**: TOML (`.memoryrc`)
- **TOML parser**: `tomllib` (Python 3.11+) or `tomli` fallback
- **Testing**: `pytest`, `pytest-asyncio`, `pytest-cov`
- **Linting / formatting**: `ruff` (no local config file; uses defaults)
- **Optional LLM client**: `litellm` or direct `httpx` to OpenAI-compatible APIs
- **Optional integrations**: `langgraph`, `langchain-core`, `mcp`, `opentelemetry`

## Project Structure

```
memctrl/
├── __init__.py          # Public API exports
├── cli.py               # Typer CLI (commands: init, add, query, tree, serve, etc.)
├── store.py             # SQLite data layer: Memory, MemoryStore, TreeNode, TriggerLog
├── tree.py              # PageIndex-style hierarchical tree builder
├── retriever.py         # Tree-traversal retrieval with reasoning traces
├── cache.py             # Query result cache (in-memory + SQLite persistence)
├── decay.py             # Confidence decay engine per layer
├── reflection.py        # Auto-detect session end and consolidate memories
├── rules.py             # .memoryrc TOML parser and rule engine (hot-reload)
├── extractor.py         # LLM-powered memory extraction with confidence scoring
├── sanitize.py          # Secret / PII redaction utilities
├── provenance.py        # Retrieval provenance tracking
├── span.py              # MemorySpan context manager for operation tracing
├── otel_exporter.py     # OpenTelemetry GenAI memory span exporter
├── mcp_server.py        # MCP server (stdio transport)
├── llm_client.py        # LiteLLM / httpx wrapper for CLI LLM calls
├── installer.py         # SKILL.md installer for AI coding assistants
├── doctor.py            # Health analysis for memory stores
├── integrations/
│   └── langgraph.py     # MemCtrlMemory, MemCtrlSaver, MemoryNode
└── templates/
    └── SKILL.md         # Template installed by `memctrl install`

tests/                   # pytest suite (470+ tests)
examples/                # Demo scripts: coding_agent_demo.py, killer_demo.py
benchmarks/              # retention_benchmark.py
docs/                    # gen-ai-memory-otel-spec.md, HTML visualizations
```

## Build and Test Commands

```bash
# Install in editable mode with all extras
pip install -e ".[llm,dev]"
# or via Makefile:
make install

# Run the full test suite
pytest tests/ -v
# or via Makefile:
make test

# Run with coverage (as CI does)
pytest tests/ -v --cov=memctrl --cov-report=xml --cov-report=term

# Lint / format checks (CI uses ruff)
ruff check memctrl/ tests/
ruff format --check memctrl/ tests/

# Build distribution
python -m build

# Clean build artifacts
make clean
```

## Running the Project

```bash
# CLI entry point (installed via pyproject.toml [project.scripts])
memctrl --version
memctrl init          # creates .memoryrc + .memctrl/ in current directory
memctrl add "fact"    # store memory (default layer: session)
memctrl query "question"
memctrl tree
memctrl serve         # start MCP server (stdio transport)
memctrl doctor        # health report
memctrl trigger on_session_end
```

```python
# Programmatic usage
from memctrl.store import MemoryStore
from memctrl.retriever import MemoryRetriever
from memctrl.otel_exporter import MemoryOTelExporter

store = MemoryStore(".memctrl/memories.db")
mid = store.insert_memory("project", "we use FastAPI", source="manual")
```

## Code Style Guidelines

- **Always** use `from __future__ import annotations` at the top of every module.
- Use `dataclasses` for data models (`Memory`, `TreeNode`, `TriggerLog`, etc.).
- Use type hints throughout; prefer `Optional[str]` and explicit `List[dict]` over bare `dict`.
- Module docstrings explain the file's purpose and list public commands/APIs.
- Inline comments explain **WHY**, not **WHAT**. Use section dividers:
  ```python
  # ---------------------------------------------------------------------------
  # Section name
  # ---------------------------------------------------------------------------
  ```
- Public methods have docstrings with Args/Returns sections.
- Redaction is mandatory before any text crosses a process boundary (LLM API, export, etc.). Use `sanitize_text()` from `memctrl.sanitize`.
- Graceful degradation for optional dependencies: wrap imports in `try/except` and set a boolean flag (e.g., `HAS_MCP`, `LANGGRAPH_AVAILABLE`).
- Async/await is used for LLM calls and tree building; sync wrappers are provided for CLI convenience.

## Testing Instructions

- **Framework**: `pytest` with `pytest-asyncio` for async tests.
- **Fixtures**: Tests use temporary SQLite databases (via `tempfile.NamedTemporaryFile`) and clean them up in teardown.
- **CLI tests**: Use `typer.testing.CliRunner` with temporary working directories.
- **Coverage**: CI runs `--cov=memctrl --cov-report=xml --cov-report=term`.
- **Cross-platform**: CI tests on Ubuntu, Windows, macOS with Python 3.10–3.13.
- **Skipping**: Some `--help` tests are skipped due to a known Typer 0.15.x + Click 8.2.x compatibility issue.

### Key test files

| Test file | What it covers |
|---|---|
| `test_store.py` | SQLite CRUD, expiration, consolidation, tree nodes, trigger logs |
| `test_cli.py` | CLI commands via CliRunner (init, add, list, forget, tree, etc.) |
| `test_tree.py` / `test_tree_incremental.py` | Tree building and incremental rebuild |
| `test_retriever.py` | Retrieval logic, reasoning traces, provenance |
| `test_decay.py` | Confidence decay math and rules |
| `test_reflection.py` | Session-end detection and consolidation |
| `test_provenance.py` | Provenance tracking and serialization |
| `test_otel_exporter.py` | OTel span creation, export, JSON/OTLP, thread safety |
| `test_rules.py` | `.memoryrc` parsing, hot-reload, trigger execution |
| `test_extractor.py` | Memory extraction, security redaction, confidence scoring |
| `test_cache.py` | Query cache invalidation, TTL, tree-version tracking |
| `test_span.py` | MemorySpan context manager operations |
| `test_doctor.py` | Health report generation |
| `test_langgraph.py` | LangGraph integration (skipped if not installed) |

## Configuration

The project reads configuration from `.memoryrc` (TOML) in the working directory. Created automatically by `memctrl init`.

Example `.memoryrc`:
```toml
[layers]
project = "architecture decisions, tech stack, ADRs, why we chose X"
session = "current task, WIP, what was done this session"
user = "preferences, working style, patterns, personal rules"

[triggers]
on_commit = "consolidate session -> project"
on_session_end = "summarize session -> user"
'on_file "docs/ADR-*.md"' = "extract -> project"
'on_file "*.md"' = "extract -> project if contains decision"

[forget]
never = ["passwords", "keys", "PII", "secrets"]
after_days = { session = 7, user = 90 }

[extract]
confidence = { explicit = 1.0, inferred = 0.7, mentioned = 0.5 }
```

Hot-reload is enabled via `watchdog`: editing `.memoryrc` applies changes immediately without restart.

Environment variables (see `.env.example`):
- `MEMCTRL_DB_PATH` — override SQLite database path
- `MEMCTRL_LLM_MODEL` — LLM model for extraction/tree building
- `MEMCTRL_LLM_API_KEY` / `MEMCTRL_LLM_BASE_URL` — LLM provider config
- `MEMCTRL_LOG_LEVEL` — logging level

## Security Considerations

- **Secret redaction**: API keys, tokens, passwords, AWS keys, and private keys are detected and replaced with `[REDACTED_<LABEL>]` before storage or LLM prompts. Patterns live in `memctrl/sanitize.py`.
- **PII redaction**: Emails, SSNs, and phone numbers are sanitized via the same path.
- **Never-Forget list**: Memories containing `passwords`, `keys`, `PII`, `secrets`, etc. are blocked from auto-deletion.
- **Trusted sources**: `doctor.py` defines `TRUSTED_SOURCES` and flags memories from untrusted origins.
- **Local-only default**: All data stays in `.memctrl/memories.db`. No cloud embedding APIs are required for retrieval.
- **Memory poisoning detection**: Retrieval provenance tracks the source of every memory, enabling detection of injected/poisoned memories.

## Deployment and Distribution

- Published to PyPI as `memctrl`.
- Build: `python -m build` (uses `hatchling`).
- CI publishes to PyPI automatically on GitHub release creation (trusted publishing via OIDC).
- CLI is exposed as `memctrl` via `[project.scripts]` in `pyproject.toml`.

## CI/CD

GitHub Actions workflows:
- `.github/workflows/ci.yml` — runs tests and coverage on `push`/`pull_request` to `main`/`master` (matrix: Ubuntu/Windows/macOS × Python 3.10–3.13). Also runs `ruff check` and `ruff format --check`.
- `.github/workflows/publish.yml` — builds and publishes to PyPI on release `published` events.

## Integration Conventions

- **MCP**: `memctrl serve` starts a stdio MCP server exposing `memctrl_query`, `memctrl_add`, `memctrl_trigger`, `memctrl_tree`, `memctrl_audit`.
- **LangGraph**: `memctrl.integrations.langgraph` provides `MemCtrlMemory`, `MemCtrlSaver`, and `MemoryNode`. Optional dependency: `pip install "memctrl[langgraph]"`.
- **AI assistant skill install**: `memctrl install --tool <name>` writes `SKILL.md` into the tool's skills directory (Claude Code, Cursor, Kimi Code, Codex, AxGa, Pi). `memctrl install --project` writes to project-scoped paths.

## Useful Notes for Agents

- When modifying the CLI, update both `memctrl/cli.py` and `tests/test_cli.py`.
- When adding a new memory operation, add corresponding OTel attribute constants in `otel_exporter.py` and tests in `test_otel_exporter.py`.
- The `MemoryStore` schema is defined in `store.py` (tables: `memories`, `tree_nodes`, `trigger_logs`, `provenance`, `otel_spans`). Schema migrations are handled implicitly on init.
- Tree building can use an LLM (`MemoryTreeBuilder.llm_client`) or fall back to keyword clustering. The fallback must always work without network access.
- The project avoids external vector databases and embedding APIs; retrieval is deterministic keyword + tree traversal.
- Confidence values: `explicit = 1.0`, `inferred = 0.7`, `mentioned = 0.5`. These are defaults in `.memoryrc` and `DEFAULT_RULES` in `rules.py`.
