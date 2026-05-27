# GenAI Memory OpenTelemetry Attributes

Status: draft

MemCtrl emits memory lifecycle telemetry so agent memory can be inspected in the same tools used for model calls, retrievals, and tool execution. This draft keeps the attribute set small and compatible with OpenTelemetry GenAI naming patterns.

## Span Names

Use `gen_ai.memory.<operation>` for memory-specific lifecycle spans.

| Operation | Span name | Purpose |
|---|---|---|
| `store` | `gen_ai.memory.store` | A memory was created. |
| `retrieve` | `gen_ai.memory.retrieve` | One memory was retrieved directly. |
| `search` | `gen_ai.memory.search` | A query searched memory and returned zero or more results. |
| `update` | `gen_ai.memory.update` | Memory content, layer, tags, or confidence changed. |
| `consolidate` | `gen_ai.memory.consolidate` | Memories moved or summarized between layers. |
| `decay` | `gen_ai.memory.decay` | Confidence decreased because a memory aged without reinforcement. |
| `expire` | `gen_ai.memory.expire` | A memory crossed its configured lifetime. |
| `forget` | `gen_ai.memory.forget` | A memory was deleted intentionally. |

## Core Attributes

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `gen_ai.system` | string | required | Memory system name, usually `memctrl`. |
| `gen_ai.operation.name` | string | required | Operation name, such as `store` or `search`. |
| `gen_ai.memory.operation` | string | required | Memory lifecycle operation. Mirrors the span suffix. |
| `gen_ai.memory.id` | string | conditional | Stable memory identifier when one memory is affected. |
| `gen_ai.memory.layer` | string | conditional | Memory layer, such as `project`, `session`, or `user`. |
| `gen_ai.memory.type` | string | optional | `semantic`, `episodic`, or `procedural`. |
| `gen_ai.memory.confidence` | double | optional | Confidence score at operation time, from `0.0` to `1.0`. |
| `gen_ai.memory.source` | string | optional | Source label, such as `manual`, `reflection`, or `mcp`. |
| `gen_ai.memory.backend` | string | recommended | Storage backend, such as `sqlite`. |

## Retrieval Attributes

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `gen_ai.memory.query` | string | conditional | Query text or a redacted query preview. |
| `gen_ai.memory.top_k` | int | optional | Requested result count. |
| `gen_ai.memory.results_count` | int | recommended | Actual result count. |
| `gen_ai.memory.trace_path` | string[] | recommended | Tree path or reasoning path used to reach the memory. |
| `gen_ai.memory.match_reason` | string | optional | Short explanation for why a memory matched. |
| `gen_ai.memory.total_searched` | int | optional | Number of memories considered during retrieval. |

## Governance Attributes

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `gen_ai.memory.provenance.id` | string | optional | Identifier for the persisted provenance record. |
| `gen_ai.memory.provenance.coverage` | double | optional | Share of memories covered by recent provenance records. |
| `gen_ai.memory.redaction.applied` | boolean | recommended | Whether redaction changed memory content before storage/export. |
| `gen_ai.memory.risk.source` | string | optional | Risk label for untrusted or legacy memory sources. |
| `gen_ai.memory.expiration.time` | string | optional | ISO timestamp when the memory expires. |
| `gen_ai.memory.decay.reason` | string | optional | Reason confidence changed, such as `age` or `reinforcement_gap`. |

## Privacy Rules

Do not export full memory content by default. Prefer IDs, layers, confidence, source labels, trace paths, and redacted previews. Full content export should require explicit opt-in and should respect the same redaction path used before storage.

## Example

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
