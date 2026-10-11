# Chunk size ablation

We measured retrieval recall@10 on an internal set of 400 questions over 1,200 contract pages.
Embedding model: multilingual-e5-base. The reranker was disabled for this study.

| Chunk size (tokens) | Overlap (tokens) | Recall@10 | MRR |
|---|---|---|---|
| 128 | 16 | 0.64 | 0.48 |
| 256 | 32 | 0.71 | 0.55 |
| 512 | 64 | 0.78 | 0.61 |
| 1024 | 128 | 0.74 | 0.57 |

## Findings

512-token chunks with 64 tokens of overlap gave the best recall@10 (0.78).
Chunks of 1024 tokens lost precision because unrelated clauses shared a chunk.
Next step: repeat the study with the cross-encoder reranker enabled.
