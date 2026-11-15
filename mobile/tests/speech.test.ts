/** Answers read aloud (src/lib/speech.ts). */
import { describe, expect, it } from 'vitest';
import { startingAnswer, type AnswerState } from '../src/lib/answer';
import { spokenAnswer, spokenSource, spokenText } from '../src/lib/speech';

const answer = (text: string, passages: AnswerState['passages']): AnswerState => ({
  ...startingAnswer(),
  status: 'completed',
  blocks: [{ id: 'm', text }],
  passages,
});

describe('reading an answer aloud', () => {
  it('spoken citations come from citation metadata, never from parsing the text', () => {
    // The text names another paper and another page; the source said is the metadata's.
    const said = spokenAnswer(
      answer('As **page 99** of the RAG survey says, Self-RAG retrieves only when needed [1]. Reranking helps too [2][1].', [
        { n: 1, title: 'Self-RAG.pdf', path: '/papers/self-rag.pdf', page: 4, heading: null, text: '', web: false },
        { n: 2, title: 'Rerankers explained', path: 'https://example.org/rerank', page: null, heading: null, text: '', web: true },
        { n: 3, title: 'Never cited.pdf', path: '/papers/x.pdf', page: 1, heading: null, text: '', web: false },
      ]),
    );
    expect(said).toBe(
      'As page 99 of the RAG survey says, Self-RAG retrieves only when needed. Reranking helps too.\nSources: Self-RAG, page 4; Rerankers explained.',
    );
    expect(said).not.toContain('[1]');
    expect(said).not.toContain('Never cited');
  });

  it('a citation with no source is not made up, and a photo is "your photo"', () => {
    expect(spokenAnswer(answer('Nothing found [7].', []))).toBe('Nothing found.');
    expect(spokenSource({ n: 1, title: 'Your photo', path: 'photo://abc', page: null, heading: null, text: '', web: false })).toBe('your photo');
    expect(spokenText('## Steps\n- First\n- Second\n```rust\nlet x = 1;\n```\nSee [the paper](https://x.org).')).toBe('Steps\nFirst\nSecond\nSee the paper.');
  });
});
