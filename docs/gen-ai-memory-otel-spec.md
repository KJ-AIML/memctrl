# Semantic Conventions for Generative AI Memory Systems

**Status**: `Development`  
**Area**: `gen-ai`  
**Type**: Span attributes  

## Overview

This proposal defines OpenTelemetry semantic conventions for observability of **memory systems** in generative AI applications. Memory systems store, retrieve, and manage knowledge across agent sessions — distinct from model inference, embeddings, or tool execution.

While existing `gen_ai.*` conventions cover LLM completions and retrievals, they do not capture the **lifecycle of memory itself**: when a memory is stored, how it is retrieved, whether it decays or expires, and how it moves between layers (e.g., working → episodic → semantic).

These conventions enable observability backends to:

- Track memory health (stale facts, low-confidence memories, coverage gaps)
- Audit why an agent retrieved a particular memory
- Measure memory system performance (store latency, retrieval precision)
- Detect memory poisoning or drift over time

## Motivation

Agent memory is becoming critical infrastructure. Current observability tracks what the model *does* (chat, tool calls) but not what the agent *remembers*. Without memory conventions:

- **Debugging is impossible**: when an agent makes a wrong decision, operators cannot see what memories influenced it
- **Compliance is manual**: no audit trail for memory-influenced decisions
- **Optimization is blind**: no metrics for retrieval precision, cache hit rates, or memory decay

This proposal complements the [Agentic Systems meta-issue](https://github.com/open-telemetry/semantic-conventions-genai/issues/35) by providing the memory-layer attributes.

## Span Names

Memory operations SHOULD use span names in the `gen_ai.memory.<operation>` namespace.

| Operation | Span name | Description |
|---|---|---|
| `store` | `gen_ai.memory.store` | A memory was created. |
| `retrieve` | `gen_ai.memory.retrieve` | One memory was retrieved directly by ID. |
| `search` | `gen_ai.memory.search` | A query searched memory and returned zero or more results. |
| `update` | `gen_ai.memory.update` | Memory content, layer, tags, or confidence changed. |
| `consolidate` | `gen_ai.memory.consolidate` | Memories moved or summarized between layers. |
| `decay` | `gen_ai.memory.decay` | Confidence decreased because a memory aged without reinforcement. |
| `expire` | `gen_ai.memory.expire` | A memory crossed its configured lifetime and was removed. |
| `forget` | `gen_ai.memory.forget` | A memory was deleted intentionally. |

## Core Attributes

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `gen_ai.system` | string | required | Memory system name, e.g. `memctrl`, `mem0`, `zep`. |
| `gen_ai.operation.name` | string | required | Operation name, such as `store` or `search`. |
| `gen_ai.memory.operation` | string | required | Memory lifecycle operation. Mirrors the span suffix. |
| `gen_ai.memory.id` | string | conditional | Stable memory identifier when one memory is affected. [1] |
| `gen_ai.memory.layer` | string | conditional | Memory layer, e.g. `project`, `session`, `user`. [2] |
| `gen_ai.memory.type` | string | optional | Memory type: `semantic`, `episodic`, or `procedural`. |
| `gen_ai.memory.confidence` | double | optional | Confidence score at operation time, from `0.0` to `1.0`. |
| `gen_ai.memory.source` | string | optional | Source label, e.g. `manual`, `reflection`, `mcp`. |
| `gen_ai.memory.backend` | string | recommended | Storage backend, e.g. `sqlite`, `postgres`, `redis`. |

[1]: Required for `store`, `retrieve`, `update`, `forget` spans.  
[2]: Required when the operation targets a specific layer.

## Retrieval Attributes

Applies to `gen_ai.memory.search` and `gen_ai.memory.retrieve` spans.

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `gen_ai.memory.query` | string | conditional | Query text or a redacted query preview. [3] |
| `gen_ai.memory.top_k` | int | optional | Requested maximum result count. |
| `gen_ai.memory.results_count` | int | recommended | Actual number of results returned. |
| `gen_ai.memory.trace_path` | string[] | recommended | Tree path or reasoning path, e.g. `["root", "project", "auth"]`. |
| `gen_ai.memory.match_reason` | string | optional | Short explanation for why a memory matched. |
| `gen_ai.memory.total_searched` | int | optional | Number of memories considered during retrieval. |

[3]: Required for `search` spans. SHOULD be redacted if the query contains PII or secrets.

## Governance Attributes

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `gen_ai.memory.provenance.id` | string | optional | Identifier for the persisted provenance/audit record. |
| `gen_ai.memory.provenance.coverage` | double | optional | Share of memories covered by recent provenance records (0.0–1.0). |
| `gen_ai.memory.redaction.applied` | boolean | recommended | Whether redaction changed memory content before storage/export. |
| `gen_ai.memory.risk.source` | string | optional | Risk label for untrusted or legacy memory sources. |
| `gen_ai.memory.expiration.time` | string | optional | ISO-8601 timestamp when the memory expires. |
| `gen_ai.memory.decay.reason` | string | optional | Reason confidence changed, e.g. `age`, `reinforcement_gap`, `manual_adjustment`. |

## Privacy and Security

Memory content often contains sensitive information (credentials, PII, proprietary code). Instrumentation MUST follow these rules:

1. **Do not export full memory content by default.** Prefer IDs, layers, confidence, source labels, and trace paths.
2. **Redact before export.** The same redaction rules applied before storage MUST be applied before telemetry export.
3. **Opt-in for full content.** Full memory content export MUST require explicit configuration and SHOULD be limited to development/debugging environments.
4. **Query redaction.** `gen_ai.memory.query` SHOULD be truncated or hashed if it contains sensitive terms.

## Examples

### Memory search

```json
{
  "name": "gen_ai.memory.search",
  "attributes": {
    "gen_ai.system": "memctrl",
    "gen_ai.operation.name": "search",
    "gen_ai.memory.operation": "search",
    "gen_ai.memory.backend": "sqlite",
    "gen_ai.memory.layer": "project",
    "gen_ai.memory.query": "middleware order for token validation",
    "gen_ai.memory.top_k": 5,
    "gen_ai.memory.results_count": 2,
    "gen_ai.memory.trace_path": ["root", "project", "auth"],
    "gen_ai.memory.total_searched": 42
  }
}
```

### Memory store

```json
{
  "name": "gen_ai.memory.store",
  "attributes": {
    "gen_ai.system": "memctrl",
    "gen_ai.operation.name": "store",
    "gen_ai.memory.operation": "store",
    "gen_ai.memory.id": "mem-123",
    "gen_ai.memory.layer": "project",
    "gen_ai.memory.type": "semantic",
    "gen_ai.memory.confidence": 1.0,
    "gen_ai.memory.source": "manual",
    "gen_ai.memory.backend": "sqlite",
    "gen_ai.memory.redaction.applied": false
  }
}
```

### Memory decay

```json
{
  "name": "gen_ai.memory.decay",
  "attributes": {
    "gen_ai.system": "memctrl",
    "gen_ai.operation.name": "decay",
    "gen_ai.memory.operation": "decay",
    "gen_ai.memory.id": "mem-456",
    "gen_ai.memory.layer": "session",
    "gen_ai.memory.confidence": 0.35,
    "gen_ai.memory.decay.reason": "age"
  }
}
```

## Reference Implementation

[MemCtrl](https://github.com/KJ-AIML/memctrl) implements these conventions as the first reference memory observability layer. Its `MemoryOTelExporter` emits spans compatible with any OTel-compatible backend (Datadog, Grafana, Jaeger, Honeycomb).

## Prior Art

- [OpenTelemetry GenAI Semantic Conventions](https://github.com/open-telemetry/semantic-conventions/tree/main/docs/gen-ai)
- [Semantic Conventions for Generative AI Agentic Systems](https://github.com/open-telemetry/semantic-conventions-genai/issues/35)

## Compatibility

- No breaking changes — only additions
- Attribute naming follows existing `gen_ai.*` patterns
- `gen_ai.memory.*` is a new sub-namespace; no collisions with existing conventions
