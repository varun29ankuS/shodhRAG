/**
 * Conversation kinds: which kind a saved record is, the Conversations list
 * filtered by kind and grouped by date (on a fixed clock, in local time as the
 * phone groups), and what a Code answer changed and ran, from its transcript.
 */
import { describe, expect, it } from 'vitest';
import { codeWork, commandEndWords, commandOk, NO_WORK } from '../src/lib/code';
import type { ConversationSummary } from '../src/lib/desktop';
import { navigate, initialNav } from '../src/lib/nav';
import { conversationKind, matchesFilter } from '../src/lib/modes';
import { emptyWords, groupConversations } from '../src/screens/Conversations';
import { CODE_TRANSCRIPT } from './codeFixture';

describe('a conversation kind', () => {
  it('is Research for a record with no mode (an older one), and for Research', () => {
    expect(conversationKind({})).toBe('research');
    expect(conversationKind({ mode: null, agentMode: null })).toBe('research');
    expect(conversationKind({ mode: 'research' })).toBe('research');
    expect(conversationKind({ mode: 'research', agentMode: 'plan' })).toBe('research');
  });

  it('is Code for a Code record, whatever its agent mode short of Creator', () => {
    expect(conversationKind({ mode: 'code' })).toBe('code');
    expect(conversationKind({ mode: 'code', agentMode: 'acceptEdits' })).toBe('code');
  });

  it('is Creator when its agent mode is Creator, as the computer decides', () => {
    expect(conversationKind({ mode: 'code', agentMode: 'creator' })).toBe('creator');
    expect(conversationKind({ agentMode: 'creator' })).toBe('creator');
  });

  it('reads a mode it does not know as Research', () => {
    expect(conversationKind({ mode: 'hologram', agentMode: 'warp' })).toBe('research');
  });
});

// Thursday 13 November 2026, 15:00, local time.
const NOW = new Date(2026, 10, 13, 15, 0);
const at = (day: number, hour: number) => new Date(2026, 10, day, hour, 0).toISOString();

function summary(id: string, updatedAt: string, fields: Partial<ConversationSummary> = {}): ConversationSummary {
  return { id, title: `Conversation ${id}`, createdAt: updatedAt, updatedAt, pinned: false, workspaceId: null, mode: null, agentMode: null, messageCount: 2, ...fields };
}

const LIST: ConversationSummary[] = [
  summary('today-research', at(13, 9)),
  summary('today-code', at(13, 14), { mode: 'code', title: 'Fix the reranker' }),
  summary('yesterday-creator', at(12, 23), { mode: 'code', agentMode: 'creator' }),
  summary('six-days', at(7, 1), { mode: 'research' }),
  summary('seven-days', at(6, 23), { mode: 'code' }),
  summary('pinned-old', at(1, 10), { pinned: true }),
];

const labels = (groups: ReturnType<typeof groupConversations>) => groups.map(g => [g.label, g.items.map(c => c.id)]);

describe('the Conversations list', () => {
  it('groups by date: pinned, Today, Yesterday, This week (up to six days), Earlier, newest first', () => {
    expect(labels(groupConversations(LIST, '', NOW))).toEqual([
      ['Pinned', ['pinned-old']],
      ['Today', ['today-code', 'today-research']],
      ['Yesterday', ['yesterday-creator']],
      ['This week', ['six-days']],
      ['Earlier', ['seven-days']],
    ]);
  });

  it('a minute after midnight, last night is Yesterday', () => {
    const justAfter = new Date(2026, 10, 14, 0, 1);
    expect(labels(groupConversations([summary('late', at(13, 23))], '', justAfter))).toEqual([['Yesterday', ['late']]]);
  });

  it('filters by kind, records without a mode among Research', () => {
    expect(labels(groupConversations(LIST, '', NOW, 'research'))).toEqual([
      ['Pinned', ['pinned-old']],
      ['Today', ['today-research']],
      ['This week', ['six-days']],
    ]);
    expect(labels(groupConversations(LIST, '', NOW, 'code'))).toEqual([
      ['Today', ['today-code']],
      ['Earlier', ['seven-days']],
    ]);
    expect(labels(groupConversations(LIST, '', NOW, 'creator'))).toEqual([['Yesterday', ['yesterday-creator']]]);
    expect(LIST.filter(c => matchesFilter(c, 'all'))).toHaveLength(LIST.length);
  });

  it('searches titles within the kind shown', () => {
    expect(labels(groupConversations(LIST, 'RERANKER', NOW, 'code'))).toEqual([['Today', ['today-code']]]);
    expect(groupConversations(LIST, 'reranker', NOW, 'research')).toEqual([]);
  });

  it('says why it is empty', () => {
    expect(emptyWords(0, 0, '', 'all')).toBe('No conversations yet. Ask a question to start one.');
    expect(emptyWords(3, 0, '', 'creator')).toBe('No Creator conversations yet.');
    expect(emptyWords(3, 2, 'zebra', 'code')).toBe('No Code conversation title has "zebra".');
    expect(emptyWords(3, 3, 'zebra', 'all')).toBe('No conversation title has "zebra".');
  });

  it('keeps the filter in the screen, replaced without a step to go back to', () => {
    const listed = navigate(initialNav('ask'), { type: 'push', screen: { kind: 'conversations' } });
    const filtered = navigate(listed, { type: 'filter', filter: 'code' });
    expect(filtered.ask).toEqual([{ kind: 'home' }, { kind: 'conversations', filter: 'code' }]);
    // Only the Conversations list has a filter.
    expect(navigate(initialNav('ask'), { type: 'filter', filter: 'code' })).toEqual(initialNav('ask'));
  });
});

describe('a Code answer from its saved transcript', () => {
  const work = codeWork(CODE_TRANSCRIPT);

  it('lists each file changed once, in the order shown, and not a refused edit or a read', () => {
    expect(work.files).toEqual([
      { path: 'src/rerank.rs', written: false, changes: 2 },
      { path: 'tests/rerank_empty.rs', written: true, changes: 1 },
    ]);
  });

  it('lists the commands with how they ended and their output', () => {
    expect(work.commands.map(c => [c.command, c.end, c.exitCode, c.output, c.outputCut])).toEqual([
      ['cargo test --test rerank_empty', 'done', 0, 'test result: ok. 3 passed; 0 failed', false],
      ['cargo clippy -- -D warnings', 'done', 101, 'error: unused variable `x`', true],
    ]);
    expect(work.commands.map(commandEndWords)).toEqual(['Exit 0', 'Failed, exit 101']);
    expect(work.commands.map(commandOk)).toEqual([true, false]);
  });

  it('a command without its run (refused before it started) still shows, from its call', () => {
    const refused = codeWork({ steps: { r: { tool: 'run_command', status: 'failed', args: { command: 'rm -rf build' }, summary: 'Refused: outside the folder' } } });
    expect(refused.commands).toEqual([
      { stepId: 'r', command: 'rm -rf build', end: 'failed', exitCode: null, output: '', outputCut: false, note: 'Refused: outside the folder' },
    ]);
  });

  it('a step saved while waiting says it waits, and nothing is approvable', () => {
    const waiting = codeWork({ steps: { w: { tool: 'edit', status: 'awaiting_approval', label: 'Edit main.rs', args: { path: 'main.rs' } } } });
    expect(waiting.waiting).toEqual([{ stepId: 'w', label: 'Edit main.rs' }]);
    expect(waiting.files).toEqual([]);
  });

  it("reads Creator's finish summary, and nothing from an answer without a transcript", () => {
    const made = codeWork({ steps: { f: { tool: 'finish_creation', status: 'done', detail: { summary: ' A poster. ', choices: [] } } } });
    expect(made.finished).toBe('A poster.');
    expect(codeWork(undefined)).toBe(NO_WORK);
    expect(codeWork('text')).toBe(NO_WORK);
  });
});
