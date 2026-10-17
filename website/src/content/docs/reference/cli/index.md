---
title: CLI reference
description: "Every memctrl command and option, generated from memctrl 2.7.0."
sidebar:
  label: Overview
  order: 0
editUrl: false
---

memctrl is one binary. Every command prints JSON on stdout, except the Markdown session brief and `--format` variants, and reports errors on stderr with a nonzero exit status. Run `memctrl <command> --help` for the same information in your terminal.

## Set up

| Command | What it does |
| --- | --- |
| [`memctrl init`](/memctrl/reference/cli/init/) | Create rules and initialize the database |
| [`memctrl install`](/memctrl/reference/cli/install/) | Install SKILL.md for a coding tool (and optionally a Claude Code session-start hook) |
| [`memctrl serve`](/memctrl/reference/cli/serve/) | Start the official MCP SDK stdio server |

## Everyday use

| Command | What it does |
| --- | --- |
| [`memctrl add`](/memctrl/reference/cli/add/) | Store a memory (or explicitly request LLM extraction) |
| [`memctrl query`](/memctrl/reference/cli/query/) | Retrieve relevant active memories with provenance |
| [`memctrl context`](/memctrl/reference/cli/context/) | Print a budgeted brief of the highest-trust memories for a session start |
| [`memctrl recall`](/memctrl/reference/cli/recall/) | The memories that match a prompt, minus what this session already saw (for a prompt hook) |
| [`memctrl list`](/memctrl/reference/cli/list/) | List memories |
| [`memctrl show`](/memctrl/reference/cli/show/) | Inspect a memory, relations and evidence |

## Review and correct

| Command | What it does |
| --- | --- |
| [`memctrl candidates`](/memctrl/reference/cli/candidates/) | List candidate memories awaiting review |
| [`memctrl accept`](/memctrl/reference/cli/accept/) | Accept a candidate into active knowledge |
| [`memctrl reject`](/memctrl/reference/cli/reject/) | Reject a memory |
| [`memctrl dispute`](/memctrl/reference/cli/dispute/) | Mark a memory as disputed |
| [`memctrl refute`](/memctrl/reference/cli/refute/) | Refute a memory; refuted claims cannot be accepted |
| [`memctrl supersede`](/memctrl/reference/cli/supersede/) | Supersede an old memory with its replacement |
| [`memctrl forget`](/memctrl/reference/cli/forget/) | Remove a memory by ID |
| [`memctrl clear`](/memctrl/reference/cli/clear/) | Clear memories; requires --yes in this noninteractive JSON CLI |
| [`memctrl history`](/memctrl/reference/cli/history/) | Show relationship lineage and evidence |

## Code citations

| Command | What it does |
| --- | --- |
| [`memctrl cite`](/memctrl/reference/cli/cite/) | Link an existing memory to the code it describes |
| [`memctrl stale`](/memctrl/reference/cli/stale/) | List memories whose cited code changed or disappeared |
| [`memctrl refresh`](/memctrl/reference/cli/refresh/) | Re-anchor a memory's citations to the current code after re-checking the memory |

## Team sharing

| Command | What it does |
| --- | --- |
| [`memctrl export`](/memctrl/reference/cli/export/) | Write shared memory to a Markdown file for review in pull requests |
| [`memctrl sync`](/memctrl/reference/cli/sync/) | After pulling: import memory.md (new entries as candidates, retractions applied), then export your store merged with it |
| [`memctrl import`](/memctrl/reference/cli/import/) | Import a memctrl export; never overwrites local memories. New entries arrive as candidates unless you pass --accept after reading the file's changes |
| [`memctrl check`](/memctrl/reference/cli/check/) | Validate memory.md without a database (for CI): format, ids, hashes, layout, and with --base that no retraction was dropped |

## Search by meaning

| Command | What it does |
| --- | --- |
| [`memctrl semantic`](/memctrl/reference/cli/semantic/) | Optional semantic search, set up under [semantic] in your user config |

## Measure and maintain

| Command | What it does |
| --- | --- |
| [`memctrl eval`](/memctrl/reference/cli/eval/) | Measure retrieval quality on a suite (built in, or --file) without touching your database |
| [`memctrl doctor`](/memctrl/reference/cli/doctor/) | Report store health (always JSON) |
| [`memctrl stats`](/memctrl/reference/cli/stats/) | Show store statistics |
| [`memctrl maintenance`](/memctrl/reference/cli/maintenance/) | Checkpoint SQLite WAL and report store statistics |
| [`memctrl decay`](/memctrl/reference/cli/decay/) | Run confidence decay or inspect low-confidence records |
| [`memctrl reinforce`](/memctrl/reference/cli/reinforce/) | Reinforce a memory's confidence |

## Structure and automation

| Command | What it does |
| --- | --- |
| [`memctrl tree`](/memctrl/reference/cli/tree/) | Show the memory tree (layers, clusters, counts); --rebuild builds it again |
| [`memctrl trigger`](/memctrl/reference/cli/trigger/) | Fire a configured trigger |
| [`memctrl audit`](/memctrl/reference/cli/audit/) | Show trigger audit records |
| [`memctrl reflect`](/memctrl/reference/cli/reflect/) | Reflect when maintenance heuristics permit |
| [`memctrl done`](/memctrl/reference/cli/done/) | Explicit session end: force reflection |
| [`memctrl review`](/memctrl/reference/cli/review/) | Review external task history |
| [`memctrl agent`](/memctrl/reference/cli/agent/) | JSON agent checkpoint and memory-node APIs |

## Observability

| Command | What it does |
| --- | --- |
| [`memctrl provenance`](/memctrl/reference/cli/provenance/) | Show retrieval provenance |
| [`memctrl heatmap`](/memctrl/reference/cli/heatmap/) | Show memory access frequencies |
| [`memctrl timeline`](/memctrl/reference/cli/timeline/) | Show recent activity |
| [`memctrl spans`](/memctrl/reference/cli/spans/) | Inspect persisted operation spans |
| [`memctrl otel-export`](/memctrl/reference/cli/otel-export/) | Export persisted spans as JSON or OTLP JSON |
| [`memctrl otel-stats`](/memctrl/reference/cli/otel-stats/) | Show operation telemetry statistics |

## Global options

Every command takes these two options. Without them, memctrl walks up from the current folder to the repository root to find `.memoryrc`, and uses `.memctrl/memories.db` in the project folder.

| Option | Description | Environment |
| --- | --- | --- |
| `--db <DB>` | SQLite database (otherwise `db_path` in `.memoryrc`, else `.memctrl/memories.db` in the project folder) | `MEMCTRL_DB_PATH` |
| `--rc <RC>` | TOML rules file (default: the nearest `.memoryrc` in this folder or a parent, up to the repository root) | |
| `-h, --help` | Print help | |
| `-V, --version` | Print version | |

<sub>Generated from `memctrl 2.7.0` by `website/scripts/gen-cli.mjs`.</sub>
