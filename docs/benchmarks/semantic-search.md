# Search by meaning (2.4)

memctrl 2.4 can add matches by meaning to its word search, using an embedding model you run. This page measures what that buys, where it doesn't help, and what it costs. Raw numbers, including every failed question, are in `semantic-search-2.4.json`.

**In short:** questions worded unlike their memory are found more often, mostly in Thai and in paraphrases (Thai held-out 92% → 100% in the top 5). Vague questions are declined a little less often: one more question each on two suites. In the agent benchmark nothing changed, because its requests share words with the memories they need: the prompt hook added exactly the same memories with and without meaning. The env-var preference the agents miss can't be reached this way, because it isn't similar in meaning to the request (0.29).

## Setup

- **Model:** EmbeddingGemma 300M, 4-bit ONNX, served over the OpenAI-compatible `/v1/embeddings` API on the same machine (the model and weights from the [retrieval comparison](retrieval-comparison.md)). Ollama and LM Studio serve the same API.
- **Settings:** the shipped EmbeddingGemma values. A memory found by meaning alone needs similarity 0.455 in `query`, and 0.56 for the prompt hook. `memctrl semantic tune` measures both on the built-in suite: 0.455 balances finding answers against declining unanswerable questions, and 0.56 is just above the closest any unanswerable question came to a memory.
- **Fusion:** `query` merges the lexical ranking (top 10) with up to 10 matches by meaning by reciprocal rank fusion (k = 60). Ties go to the lexical match.
- **Suites:** the five suites in `bench/retrieval/suites/`, k = 5.

## `query`

| Suite | Words only: hit@1 | hit@5 | Declines vague | + meaning: hit@1 | hit@5 | Declines vague |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Real-dev (77 memories, 60 questions) | 80.0% | 91.7% | | 80.0% | **95.0%** | |
| Thai team (56 memories, 104 questions) | 92.7% | 95.1% | 100% | **96.3%** | **96.3%** | 100% |
| Thai held-out (25 questions) | 83.3% | 91.7% | 92.3% | **100%** | **100%** | 84.6% |
| English mirror (vague and off-topic) | | | 91.7% | | | 91.7% |
| Built-in (34 memories, 56 questions) | 93.0% | 93.0% | 92.3% | **95.3%** | **95.3%** | 84.6% |

hit@5 and the decline rates match the comparison's "memctrl + EmbeddingGemma, rank fusion" row on every suite. Each drop in declines is one question.

## The prompt hook

`memctrl eval --retriever recall` runs each question through the prompt hook's logic.

| Suite | Words only: answer added | Nothing added to vague | + meaning: answer added | Nothing added to vague |
| --- | ---: | ---: | ---: | ---: |
| Real-dev | 81.7% | | **85.0%** | |
| Thai team | 73.2% | 100% | 73.2% | 100% |
| Thai held-out | 66.7% | 100% | 66.7% | 100% |
| English mirror | | 91.7% | | 91.7% |
| Built-in | 72.1% | 100% | **74.4%** | 100% |

The hook's bar is higher than `query`'s because nobody asked: 27 of the 60 real-dev answers clear it, and so the gain is smaller. On these suites it added nothing to any vague question that words alone left alone. The Thai suites gain nothing in the hook: no Thai answer that words missed comes close enough.

**Noise.** On 15 everyday prompts unrelated to the stored memories (`agent_bench probe`), the closest any came to a real-dev memory was 0.54, under 0.56. The hook added the same 2 memories as with words only, on the 70-memory store and on the 1,069-memory store.

An earlier version also let a memory in on one shared word plus similarity above 0.455. On the probe it added two unrelated notes to "Fix the typo in the comment at the top of server.ts" and nothing useful to any task, so it was removed.

## Agent benchmark

One round of the 16 tasks from [agent-benchmark.md](agent-benchmark.md), same harness and judge, with semantic search on:

| Setup | Tasks right | Cost | Matches by meaning in the hook |
| --- | ---: | ---: | ---: |
| memctrl 2.4 + EmbeddingGemma, 70 memories | 14/16 | $1.04 | 0 |
| memctrl 2.4 + EmbeddingGemma, 1,069 memories | 14/16 | $1.10 | 0 |
| memctrl 2.3.0 release (for reference, an earlier round) | 14/16 and 14/16 | $1.12, $1.08 | — |

The probe predicted this: for every task the hook added exactly the memories it adds without meaning, because each task's request shares words with the memory it needs. The same two tasks failed. In env-var, the agent never hears that the user wants new env vars documented in the README; in idem-cleanup, the agent is told the team rejected the cleanup and builds it anyway. The session-start hook added context in all 32 sessions and the prompt hook in 30; the two without were format-amount, where it adds nothing in 2.3 either. No session had an error.

## What meaning doesn't reach

The env-var task asks to "Add a BODY_LIMIT_BYTES env var (default 1048576) and use it as Fastify's bodyLimit". The user-layer preference "Wants every new env var documented in README's Configuration table in the same PR" applies, but the link is an inference: adding an env var means documenting it. EmbeddingGemma puts the two at 0.29 (0.31 for the whole prompt), far below either threshold and below many unrelated pairs. No similarity threshold separates it from noise. A rule like that belongs in the brief, or in CLAUDE.md.

## Requests worded unlike their memories

The agent tasks above share words with their memories, so they can't show what meaning adds. [Eight tasks written to avoid those words](reworded-tasks.md), 4 of them in Thai, can. The prompt hook added none of the memories they need, with or without meaning. EmbeddingGemma put the needed memory first among 71 for 4 of the 8, but at similarity 0.34 to 0.49, while unrelated everyday prompts reach 0.54. No threshold separates them. What brought these memories to the agent was 2.5's larger brief.

## Another server and model

The same build against LM Studio serving `text-embedding-nomic-embed-text-v1.5`, an English model: `semantic tune` measured threshold 0.664 and recall threshold 0.75; real-dev hit@5 was 93.3% (91.7% with words only) and the English mirror still declined 91.7%. Status reported 768 dimensions and a 16 ms round trip.

## Cost

- **Time.** A search adds one request to the model: in process, about 8 ms with the local EmbeddingGemma above (word search alone is 0.2 ms), and the CLI goes from about 9 ms to about 18 ms with LM Studio. Embedding 1,070 memories took 15 s; 70 took 1 s. Searches embed up to 16 memories that have no vector themselves.
- **Space.** 3 KB per memory at 768 dimensions; `dimensions = 256` keeps a third for Matryoshka models such as EmbeddingGemma.
- **With semantic search off,** 2.4 returns what 2.3 returned on all five suites, and `list`, `query` and `context` start as fast (10.5, 10.2 and 12.0 ms against 10.2, 10.1 and 11.9).

## Reproduce

Start a server with an embedding model (`ollama pull embeddinggemma`, or load one in LM Studio), add `[semantic]` to your user config, then:

```bash
memctrl semantic status
memctrl eval --file bench/retrieval/suites/realdev.json --retriever semantic
memctrl eval --file bench/retrieval/suites/realdev.json --retriever recall
MEMCTRL_SEMANTIC=off memctrl eval --file bench/retrieval/suites/realdev.json --retriever recall
```

Ollama's and LM Studio's EmbeddingGemma builds are quantized differently from the ONNX weights used here, so expect similarities to move by a few hundredths; `memctrl semantic tune` measures yours. For the agent benchmark, pass `--semantic` to `agent_bench setup` (see `bench/README.md`).
