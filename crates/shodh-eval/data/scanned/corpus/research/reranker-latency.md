# Reranker latency on CPU

We timed cross-encoder reranking of 50 candidates per query on a 4-core laptop CPU.

| Model | Batch size | Latency per query (ms) | nDCG@10 gain |
|---|---|---|---|
| MiniLM-L6 | 16 | 180 | +0.06 |
| MiniLM-L12 | 16 | 340 | +0.07 |
| BGE-reranker-base | 8 | 910 | +0.09 |

## Decision

We ship MiniLM-L6 because it keeps reranking under 200 ms per query.
The larger models gain little ranking quality for two to five times the latency.
