/**
 * A synthetic long conversation for measuring the Ask view: user questions
 * and agent answers that mix long markdown, code, citations, charts and
 * tables, in the shape `load_conversations` returns. Deterministic, so runs
 * before and after a change compare like with like. Synthetic text only.
 */
import type { Conversation, ConversationMessage } from '../src/hooks/useConversations';

const TOPICS = [
  'product quantization', 'HNSW graph construction', 'IVF partitioning', 're-ranking with cross-encoders',
  'chunking strategies', 'hybrid BM25 and dense retrieval', 'reciprocal rank fusion', 'embedding drift',
  'quantized storage', 'filtered search', 'recall at k', 'latency budgets',
];

const SENTENCES = [
  'The method trades a small loss in recall for a large reduction in memory, which matters once the index no longer fits in RAM.',
  'Graph degree controls both build time and query latency; higher degrees improve recall but slow insertion.',
  'Partition counts near the square root of the corpus size keep probe costs balanced against list lengths.',
  'Scores from different retrievers are not comparable, so fusion by rank is more robust than fusion by score.',
  'Short chunks improve precision for factual questions while longer chunks keep the context an answer needs.',
  'Caching the decoded centroids removes most of the per-query overhead on the hot path.',
  'The benchmark ran on a single core so that the comparison reflects algorithmic cost rather than parallelism.',
  'A filtered query that excludes most of the corpus degrades graph search unless the filter is applied during traversal.',
];

function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function paragraph(rand: () => number, sentences: number, citeMax: number): string {
  const parts: string[] = [];
  for (let i = 0; i < sentences; i++) {
    let s = SENTENCES[Math.floor(rand() * SENTENCES.length)];
    if (rand() < 0.45) {
      const a = 1 + Math.floor(rand() * citeMax);
      s = s.replace(/\.$/, rand() < 0.3 ? ` [${a}][${1 + ((a) % citeMax)}].` : ` [${a}].`);
    }
    parts.push(s);
  }
  return parts.join(' ');
}

function codeBlock(i: number): string {
  const lines = [
    '```typescript',
    `export function searchIndex${i}(query: Float32Array, k: number): Hit[] {`,
    '  const candidates = new MinHeap<Hit>((a, b) => a.score - b.score);',
    '  for (const list of probeLists(query, NPROBE)) {',
    '    for (const entry of list.entries) {',
    '      const score = dot(query, decode(entry.code, codebook));',
    '      if (candidates.size < k) candidates.push({ id: entry.id, score });',
    '      else if (score > candidates.peek().score) {',
    '        candidates.pop();',
    '        candidates.push({ id: entry.id, score });',
    '      }',
    '    }',
    '  }',
    '  return candidates.toSortedArray().reverse();',
    '}',
    '',
    `const results = searchIndex${i}(embed("${TOPICS[i % TOPICS.length]}"), 10);`,
    'console.table(results.map(r => ({ id: r.id, score: r.score.toFixed(4) })));',
    '```',
  ];
  return lines.join('\n');
}

function chartBlock(i: number): string {
  const spec = {
    type: i % 20 === 0 ? 'line' : 'bar',
    title: `Recall@10 by method (run ${i})`,
    xKey: 'method',
    series: [{ key: 'recall', label: 'Recall@10' }, { key: 'latency', label: 'p95 latency (ms)' }],
    data: [
      { method: 'Flat', recall: 1.0, latency: 41.2 },
      { method: 'HNSW', recall: 0.981, latency: 1.9 },
      { method: 'IVF-PQ', recall: 0.912, latency: 0.8 },
      { method: 'IVF-Flat', recall: 0.967, latency: 3.4 },
      { method: 'ScaNN', recall: 0.975, latency: 1.1 },
    ],
  };
  return '```chart\n' + JSON.stringify(spec) + '\n```';
}

function tableBlock(i: number): string {
  const rows = [
    '| Method | Recall@10 | Memory (GB) | Build (min) |',
    '| --- | --- | --- | --- |',
    `| Flat | 1.000 | ${(12 + (i % 5)).toFixed(1)} | 0 |`,
    '| HNSW (M=32) | 0.981 | 15.8 | 42 |',
    '| IVF4096-PQ64 | 0.912 | 1.1 | 18 |',
    '| IVF4096-Flat | 0.967 | 12.4 | 9 |',
    '| ScaNN | 0.975 | 3.2 | 25 |',
  ];
  return rows.join('\n');
}

function answerText(i: number, rand: () => number, passages: number): string {
  const topic = TOPICS[i % TOPICS.length];
  const blocks: string[] = [];
  blocks.push(`## ${topic[0].toUpperCase()}${topic.slice(1)}`);
  blocks.push(paragraph(rand, 5, passages));
  blocks.push('### What the sources say');
  blocks.push(['- ' + paragraph(rand, 1, passages), '- ' + paragraph(rand, 1, passages), '- **Trade-off:** ' + paragraph(rand, 1, passages)].join('\n'));
  if (i % 4 === 1) blocks.push(codeBlock(i));
  if (i % 7 === 3) blocks.push(tableBlock(i));
  if (i % 10 === 5) blocks.push(chartBlock(i));
  blocks.push(paragraph(rand, 4, passages));
  if (i % 3 === 0) {
    blocks.push('### Steps');
    blocks.push(['1. ' + paragraph(rand, 1, passages), '2. ' + paragraph(rand, 1, passages), '3. ' + paragraph(rand, 1, passages)].join('\n'));
  }
  blocks.push(paragraph(rand, 3, passages));
  return blocks.join('\n\n');
}

function transcriptFor(i: number, text: string, startedAtMs: number, passages: number): Record<string, unknown> {
  const runId = `run-perf-${i}`;
  const stepId = `step-${i}`;
  return {
    runId,
    sessionId: 'sess-perf',
    model: 'synthetic/model',
    warning: null,
    status: 'completed',
    startedAtMs,
    durationMs: 6400,
    error: null,
    errorCode: null,
    blocks: [
      { kind: 'step', stepId },
      { kind: 'text', id: `text-${i}`, text },
    ],
    steps: {
      [stepId]: {
        id: stepId,
        parentId: null,
        tool: 'search_documents',
        label: `Searched your library for “${TOPICS[i % TOPICS.length]}”`,
        args: { query: TOPICS[i % TOPICS.length] },
        tier: 'read',
        startedAtMs: startedAtMs + 200,
        status: 'done',
        summary: `${passages} passages`,
        detail: null,
        durationMs: 380,
        progress: null,
        approval: null,
        children: [],
      },
    },
    plan: null,
    usage: { inputTokens: 5200, outputTokens: 900, cacheReadTokens: 0, costUsd: null },
    passages: Array.from({ length: passages }, (_, k) => ({
      n: k + 1,
      file: `synthetic-paper-${(i + k) % 9}.pdf`,
      path: `C:/synthetic/papers/synthetic-paper-${(i + k) % 9}.pdf`,
      page: String(1 + ((i * 3 + k) % 14)),
      heading: `Section ${1 + (k % 5)}`,
      score: 0.9 - k * 0.05,
      text: SENTENCES[(i + k) % SENTENCES.length],
    })),
    groundings: [],
    interimIds: [],
  };
}

/** `turns` question/answer pairs (2 × turns messages), oldest first. */
export function longConversation(turns: number, id = 'conv-perf-long'): Conversation {
  const rand = mulberry32(42);
  const base = Date.UTC(2026, 8, 1, 9, 0, 0);
  const messages: ConversationMessage[] = [];
  for (let i = 0; i < turns; i++) {
    const at = base + i * 120_000;
    messages.push({
      id: `msg-u-${i}`,
      role: 'user',
      content: `Question ${i + 1}: how does ${TOPICS[i % TOPICS.length]} affect recall and memory in my papers?`,
      timestamp: new Date(at).toISOString(),
    });
    const passages = 4 + (i % 4);
    const text = answerText(i, rand, passages);
    messages.push({
      id: `msg-a-${i}`,
      role: 'assistant',
      content: text,
      timestamp: new Date(at + 6400).toISOString(),
      transcript: transcriptFor(i, text, at + 1000, passages),
    });
  }
  return {
    id,
    title: `Synthetic long conversation (${messages.length} messages)`,
    messages,
    createdAt: new Date(base).toISOString(),
    updatedAt: new Date(base + turns * 120_000).toISOString(),
    pinned: false,
  };
}

/** A short conversation shown first, so opening the long one can be timed. */
export function shortConversation(): Conversation {
  const now = new Date(Date.UTC(2026, 9, 1)).toISOString();
  return {
    id: 'conv-perf-short',
    title: 'Synthetic short conversation',
    messages: [
      { id: 'msg-s-u', role: 'user', content: 'What is reciprocal rank fusion?', timestamp: now },
      { id: 'msg-s-a', role: 'assistant', content: 'It merges ranked lists by summing 1/(k + rank) per document.', timestamp: now },
    ],
    createdAt: now,
    updatedAt: now,
    pinned: true,
  };
}
