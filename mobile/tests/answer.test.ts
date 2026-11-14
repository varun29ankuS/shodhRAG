import { describe, expect, it } from 'vitest';
import { answerText, citedNumbers, foldAnswer, passagesFromDetail, savedAnswer, startingAnswer, type AnswerState } from '../src/lib/answer';
import { parseAnswer, parseInline, snippet } from '../src/lib/markdown';

const fold = (events: unknown[], state: AnswerState = startingAnswer()) => events.reduce<AnswerState>(foldAnswer, state);

describe('an answer folded from the computer\'s events', () => {
  it('keeps the answer and drops working notes, replaced drafts and other runs', () => {
    const state = fold([
      { type: 'run_started', runId: 'r1' },
      { type: 'text_delta', runId: 'r1', messageId: 'note', delta: "I'll search your papers." },
      { type: 'text_interim', runId: 'r1', messageId: 'note', interim: true },
      { type: 'step_started', runId: 'r1', stepId: 's1', label: 'Searching your library' },
      {
        type: 'step_finished',
        runId: 'r1',
        stepId: 's1',
        detail: {
          passages: [
            { n: 1, path: 'c:/papers/self-rag.pdf', display_path: 'C:/Papers/Self-RAG.pdf', file: 'Self-RAG.pdf', page: '3-4', heading: 'Method', text: 'Retrieve token.' },
            { n: 0, path: 'bad' },
          ],
          webSources: [{ n: 2, title: 'arXiv', url: 'https://arxiv.org/abs/2310.11511', snippet: 'Self-RAG' }, { n: 3, url: 'javascript:alert(1)' }],
        },
      },
      { type: 'text_delta', runId: 'r1', messageId: 'draft', delta: 'A first draft.' },
      { type: 'text_delta', runId: 'r1', messageId: 'final', delta: 'It retrieves on demand [1].' },
      { type: 'text_delta', runId: 'other', messageId: 'x', delta: 'Not this run.' },
      { type: 'grounding', runId: 'r1', report: { supersededMessageIds: ['draft'] } },
    ], { ...startingAnswer(), runId: 'r1' });

    expect(answerText(state)).toBe('It retrieves on demand [1].');
    expect(state.status).toBe('running');
    expect(state.doing).toBeNull();
    expect(state.passages).toEqual([
      { n: 1, title: 'Self-RAG.pdf', path: 'C:/Papers/Self-RAG.pdf', page: 3, heading: 'Method', text: 'Retrieve token.', web: false },
      { n: 2, title: 'arXiv', path: 'https://arxiv.org/abs/2310.11511', page: null, heading: null, text: 'Self-RAG', web: true },
    ]);
    const done = foldAnswer(state, { type: 'run_finished', runId: 'r1', status: 'completed', error: null });
    expect(done.status).toBe('completed');
  });

  it('lists the steps waiting for an approval until they finish', () => {
    const asked = fold([{ type: 'approval_requested', runId: 'r1', stepId: 'fetch-1', tool: 'fetch_url', label: 'Fetch arxiv.org' }]);
    expect(asked.waiting).toEqual([{ stepId: 'fetch-1', label: 'Fetch arxiv.org', tool: 'fetch_url' }]);
    expect(fold([{ type: 'step_finished', runId: 'r1', stepId: 'fetch-1', detail: null }], asked).waiting).toEqual([]);
  });

  it('reads an answer the computer saved, and one saved while it was written', () => {
    const transcript = {
      runId: 'r1',
      status: 'completed',
      blocks: [
        { kind: 'text', id: 'note', text: 'Searching.' },
        { kind: 'step', stepId: 's1' },
        { kind: 'text', id: 'a', text: 'Self-RAG grades passages [2].' },
      ],
      interimIds: ['note'],
      groundings: [],
      passages: [{ n: 2, file: 'self-rag.pdf', path: 'c:/p/self-rag.pdf', displayPath: 'C:/P/self-rag.pdf', page: '4', heading: null, text: 'Critique tokens.' }],
    };
    const saved = savedAnswer('Self-RAG grades passages [2].', transcript);
    expect(answerText(saved)).toBe('Self-RAG grades passages [2].');
    expect(saved.passages[0]).toMatchObject({ n: 2, path: 'C:/P/self-rag.pdf', page: 4, web: false });
    expect(saved.status).toBe('completed');
    expect(savedAnswer('', { ...transcript, status: 'running' }).status).toBe('running');
    // Without a transcript, the saved text is the answer.
    expect(answerText(savedAnswer('Just text.', undefined))).toBe('Just text.');
  });

  it('finds the sources an answer cites, in order', () => {
    expect(citedNumbers('First [2], then [1, 3] and [2] again; [10].')).toEqual([2, 1, 3, 10]);
    expect(passagesFromDetail(null)).toEqual([]);
  });
});

describe("the answer's structure", () => {
  it('reads headings, lists, code, quotes and tables', () => {
    const blocks = parseAnswer(['It decides itself [1].', '', '## How it works', '- **Retrieve.** On demand [1]', '2. Then grade', '> A quote', '```', 'let x = 1;', '```', '| a | b |', '|---|---|', '| 1 | 2 |'].join('\n'));
    expect(blocks.map(b => b.kind)).toEqual(['p', 'h', 'li', 'li', 'quote', 'code', 'table']);
    expect(blocks[0]).toEqual({
      kind: 'p',
      inlines: [{ kind: 'text', text: 'It decides itself ', strong: false, em: false, code: false }, { kind: 'cite', n: 1 }, { kind: 'text', text: '.', strong: false, em: false, code: false }],
    });
    expect(blocks[2]).toMatchObject({ kind: 'li', marker: '', inlines: [{ kind: 'text', text: 'Retrieve.', strong: true }, { kind: 'text', text: ' On demand ' }, { kind: 'cite', n: 1 }] });
    expect(blocks[3]).toMatchObject({ kind: 'li', marker: '2.' });
    expect(blocks[5]).toEqual({ kind: 'code', text: 'let x = 1;' });
    expect(blocks[6]).toMatchObject({ kind: 'table' });
    if (blocks[6].kind === 'table') expect(blocks[6].rows).toHaveLength(2);
  });

  it('reads text that is still streaming', () => {
    // An unclosed fence runs to the end; an unclosed ** waits.
    expect(parseAnswer('```\nfn main(')).toEqual([{ kind: 'code', text: 'fn main(' }]);
    expect(parseInline('**Retr')).toEqual([{ kind: 'text', text: 'Retr', strong: false, em: false, code: false }]);
    expect(parseInline('`code')).toEqual([{ kind: 'text', text: '`code', strong: false, em: false, code: false }]);
    expect(parseInline('see [the paper](https://x.org) [1, 2]')).toEqual([
      { kind: 'text', text: 'see the paper ', strong: false, em: false, code: false },
      { kind: 'cite', n: 1 },
      { kind: 'cite', n: 2 },
    ]);
  });

  it('makes a plain snippet', () => {
    expect(snippet('## Title\n\nIt **retrieves** on demand [1].', 60)).toBe('Title It retrieves on demand.');
    expect(snippet('a'.repeat(200), 20)).toHaveLength(20);
  });
});
