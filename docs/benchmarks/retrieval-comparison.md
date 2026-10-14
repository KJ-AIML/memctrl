# Retrieval compared with other memory tools

Each system got the same memories and the same questions and was scored by the same code (`bench/retrieval/compare.py`). There was no network and no LLM. Raw numbers are in `retrieval-comparison-2.2.json`, measured with memctrl 2.2.0. 2.3 returns identical results for `query`; its new `recall` uses the same search.

**What was compared**

- **memctrl** 2.2.0 and 2.1.0.
- **basic-memory 0.23.2**, in text, vector and hybrid modes. Hybrid is its default, and its default embedding model is English.
- **Keyword grep**, the way Letta and Cline tools search notes.
- **Vector search**, the retrieval method Mem0, Zep and Chroma use. It ran on two local multilingual models: multilingual-e5-small and EmbeddingGemma 300M. Each returned its top 5, with and without a similarity cutoff, and the cutoff was tuned on the built-in suite only. Mem0 and Zep themselves weren't run, because their defaults need an LLM and a paid embedding API.
- **memctrl plus an embedding model**, as a fallback and as a rank fusion.
- **Load everything**, how Claude Code's auto memory and a hand-kept CLAUDE.md work.

**Suites**

- **Real-dev:** 77 memories and 60 questions from the ledger-api project.
- **Thai team:** 56 memories and 104 questions, plus 25 held-out questions that were never used for tuning.
- **English mirror:** vague and off-topic questions.
- **Built-in:** `dev-memory-v2`.

## Main results

hit@5 is the share of answerable questions whose memory is in the top 5. "Says nothing" is the share of vague or off-topic questions answered with nothing. Chars is the memory text returned per real-dev question.

| System | Real-dev hit@5 | Thai hit@5 | Thai held-out hit@5 | Thai: says nothing | English: says nothing | Chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| **memctrl 2.2** | 92% | 95% | 92% | **100%** | **92%** | 299 |
| memctrl 2.1 | 92% | 94% | 92% | 59% | 50% | 304 |
| basic-memory, default (hybrid) | 95% | 63% | 58% | 0% | 0% | 616 |
| basic-memory, text | 78% | 40% | 17% | 95% | 42% | 321 |
| Keyword grep | 92% | 83% | 42% | 95% | 42% | 448 |
| Vector top 5 (EmbeddingGemma) | 98% | 98% | 100% | 0% | 0% | 654 |
| Vector with cutoff (EmbeddingGemma) | 80% | 55% | 92% | 100% | 100% | 212 |
| memctrl + EmbeddingGemma, rank fusion | 95% | 96% | 100% | 100% | 92% | 334 |
| Load everything | 100% | 100% | 100% | 0% | 0% | 9,267 |

- **memctrl both finds and declines.** It finds the memory about as often as the others, and it is the only system here that also answers vague questions with nothing, in Thai and English.
- **Vector search finds paraphrases but never declines.** It always returns 5 notes, about twice the text. A cutoff stops that, but on the Thai suite it drops recall to 55%.
- **basic-memory's default is weak in Thai**, because its embedding model is English.
- **memctrl plus embeddings**, as rank fusion, closes the paraphrase gap and still declines vague Thai questions. Held-out vague questions drop from 92% to 85%, so semantic search should stay optional.
- **Speed.** A memctrl search takes 0.14 ms in process, and a CLI call takes about 10 ms end to end. EmbeddingGemma adds about 10 ms per query. A basic-memory CLI call takes seconds, mostly Python start-up; its MCP server mode wasn't measured.

## Caveats

- **Retrieval only.** `agent-benchmark.md` measures whether the agent then does the task right.
- **Suites by AI testers, not independent people.** The real-dev questions were written by the same tester who wrote the memories, so they share wording, which favors keyword search.
- **Small stores**, with 34 to 77 memories each.
