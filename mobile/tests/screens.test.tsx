/**
 * Each screen as the phone shows it, against a scripted computer: loading,
 * empty, connected with data, and with the computer asleep. Then the theme
 * following the phone (and the user's choice over it), the menu taking the
 * app to every section, and every control's 44px size.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { App } from '../src/App';
import type { LinkStatus } from '../src/lib/link';
import { initialNav, type Nav } from '../src/lib/nav';
import { forgetPages } from '../src/lib/pages';
import { tokensCss } from '../tokens';
import { ASLEEP, computer, CONNECTED, fakeSource, setPhoneTheme, silentComputer, type FakeSource } from './phone';
import type { FakeLink } from './fakeLink';
import { CODE_CONVERSATION } from './codeFixture';

const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

const CONVERSATIONS = [
  { id: 'c1', title: 'How does Self-RAG decide when to retrieve?', createdAt: ago(5), updatedAt: ago(5), pinned: false, workspaceId: 'w1', mode: null, agentMode: null, messageCount: 2 },
  { id: 'c2', title: 'Where should the best passages go?', createdAt: ago(200), updatedAt: ago(200), pinned: true, workspaceId: null, mode: null, agentMode: null, messageCount: 4 },
  { id: 'code-1', title: 'Make the reranker skip empty passages', createdAt: ago(30), updatedAt: ago(30), pinned: false, workspaceId: 'w1', mode: 'code', agentMode: 'ask', messageCount: 2 },
  { id: 'make-1', title: 'A poster of the results', createdAt: ago(40), updatedAt: ago(40), pinned: false, workspaceId: 'w-gone', mode: 'code', agentMode: 'creator', messageCount: 2 },
];

const SAVED = {
  id: 'c1',
  title: CONVERSATIONS[0].title,
  updatedAt: ago(5),
  messages: [
    { id: 'q', role: 'user', content: 'How does Self-RAG decide when to retrieve?', timestamp: ago(5) },
    {
      id: 'a',
      role: 'assistant',
      content: 'It decides itself [1].',
      timestamp: ago(5),
      transcript: {
        runId: 'r',
        status: 'completed',
        startedAtMs: 0,
        steps: {},
        blocks: [{ kind: 'text', id: 'm', text: 'It decides itself [1].\n\n- **Retrieve token.** Only when needed [1].' }],
        passages: [{ n: 1, file: 'self-rag.pdf', path: '/papers/self-rag.pdf', page: '3', heading: null, text: 'Passages are fetched only when needed.' }],
        groundings: [],
        interimIds: [],
      },
    },
  ],
};

const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

function connectedComputer(overrides: Record<string, unknown> = {}): FakeLink {
  return computer({
    conversations_list: CONVERSATIONS,
    load_conversation: (args: Record<string, unknown>) => (args.conversationId === CODE_CONVERSATION.id ? CODE_CONVERSATION : SAVED),
    library_folders: (args: Record<string, unknown>) =>
      args.path === undefined
        ? { folders: [{ name: 'Papers', path: '/papers' }] }
        : { folders: [{ name: 'Retrieval', path: '/papers/retrieval' }], files: [{ name: 'self-rag.pdf', path: '/papers/self-rag.pdf' }, { name: 'notes.md', path: '/papers/notes.md' }] },
    approvals_pending: [
      { sessionId: 's1', stepId: 'note-1', conversationId: 'c1', tool: 'create_note', title: 'Save a note about Self-RAG', requestedAt: ago(2), phoneMay: true },
      { sessionId: 's2', stepId: 'cmd-1', conversationId: null, tool: 'run_command', title: 'Run the test suite', requestedAt: ago(9), phoneMay: false },
    ],
    workspaces_list: [{ id: 'w1', name: 'Vector DB reading', description: '', color: '', pinned: false, chatCount: 3, lastActiveAt: ago(4) }],
    get_source_file_info: (args: Record<string, unknown>) => ({ path: args.filePath, fileName: 'notes.md', extension: 'md', sizeBytes: 10, kind: 'text' }),
    read_source_text: { text: 'Retrieve less, but better.', part: 0, parts: 1, truncated: false },
    read_source_page: (args: Record<string, unknown>) => ({ page: args.page, pages: 12, width: 320, height: 414, png: PNG }),
    agent_approve: null,
    ...overrides,
  });
}

function emptyComputer(): FakeLink {
  return connectedComputer({ conversations_list: [], library_folders: { folders: [] }, approvals_pending: [], workspaces_list: [] });
}

let source: FakeSource;
let shown: { unmount: () => void } | null = null;

/** Show the app at `nav` (the screen shown before goes). */
function show(link: FakeLink, status: LinkStatus, nav: Nav) {
  shown?.unmount();
  source = fakeSource(link, status);
  const result = render(<App source={source} start={{ screen: 'test', nav }} />);
  shown = result;
  return result;
}

const answerNav: Nav = { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'answer', conversationId: 'c1', question: null, workspaceId: null }] };
const conversationsNav: Nav = { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'conversations' }] };
const codeNav: Nav = { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'answer', conversationId: 'code-1', question: null, workspaceId: 'w1' }] };
const folderNav: Nav = { ...initialNav('library'), library: [{ path: '/papers', name: 'Papers' }] };

beforeEach(() => forgetPages());
afterEach(() => {
  forgetPages();
  shown = null;
});

const ASLEEP_WORDS = 'Your computer is asleep or offline';

describe('Home', () => {
  it('shows the greeting, the ask box, Today, the conversation to continue and the recent ones', async () => {
    show(connectedComputer(), CONNECTED, initialNav('ask'));
    expect(await screen.findByText('Continue')).toBeTruthy();
    expect(screen.getByRole('heading', { level: 1 }).textContent).toMatch(/^Good (morning|afternoon|evening)$/);
    expect(screen.getByLabelText('Ask about your library')).toBeTruthy();
    expect(screen.getByText('Your agenda, tasks and automations')).toBeTruthy();
    expect(await screen.findByText(/^It decides itself./)).toBeTruthy();
    expect(screen.getByText('Where should the best passages go?')).toBeTruthy();
    expect(await screen.findByText('What do my files in Papers cover?')).toBeTruthy();
  });

  it('while loading shows a skeleton, and with nothing yet says how to start', async () => {
    show(silentComputer(), CONNECTED, initialNav('ask'));
    expect(await screen.findByLabelText('Loading your conversations')).toBeTruthy();
    show(emptyComputer(), CONNECTED, initialNav('ask'));
    expect(await screen.findByText('No conversations yet. Ask a question above to start one.')).toBeTruthy();
  });

  it('with the computer asleep says so, offers to try again, and cannot ask', async () => {
    show(silentComputer(), ASLEEP, initialNav('ask'));
    expect(await screen.findByText(ASLEEP_WORDS)).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(source.reconnects).toBe(1));
  });
});

describe('Ask', () => {
  it('shows the question as a headline, the answer with its citations, and the pages it cites', async () => {
    show(connectedComputer(), CONNECTED, answerNav);
    expect(await screen.findByRole('heading', { level: 1, name: 'How does Self-RAG decide when to retrieve?' })).toBeTruthy();
    expect(await screen.findByText('Retrieve token.')).toBeTruthy();
    expect(screen.getAllByRole('button', { name: 'Source 1: self-rag.pdf' })).toHaveLength(2);
    expect(screen.getByText('Answered from 1 passage in your library')).toBeTruthy();
    expect(screen.getByText('From your library')).toBeTruthy();
    expect(await screen.findByAltText('self-rag.pdf, page 3')).toBeTruthy();
    // A citation opens the cited page in the Reader, with the passage.
    fireEvent.click(screen.getAllByRole('button', { name: 'Source 1: self-rag.pdf' })[0]);
    const reader = await screen.findByRole('dialog', { name: 'self-rag.pdf' });
    expect(within(reader).getByText('Passages are fetched only when needed.')).toBeTruthy();
  });

  it('streams a new question and keeps the approvals it lists answerable', async () => {
    let link!: FakeLink;
    link = connectedComputer({
      load_conversation: null,
      agent_start: 'session-9',
      agent_send: () => {
        queueMicrotask(() => {
          link.send('agent_event', { sessionId: 'session-9', event: { type: 'text_delta', runId: 'run-9', messageId: 'm', delta: 'Streaming now' } });
        });
        return 'run-9';
      },
    });
    const nav: Nav = { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'answer', conversationId: 'new', question: 'What is new?', workspaceId: null }] };
    show(link, CONNECTED, nav);
    expect(await screen.findByRole('heading', { level: 1, name: 'What is new?' })).toBeTruthy();
    expect(await screen.findByText((_, element) => element?.tagName === 'P' && element.textContent === 'Streaming now')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Stop the answer' })).toBeTruthy();
    // The listed approvals' sessions and the answer's are subscribed together.
    await waitFor(() => expect(link.subscriptions.at(-1)).toEqual(['s1', 's2', 'session-9']));
  });

  it('while loading shows a skeleton, and with the computer asleep says so', async () => {
    show(silentComputer(), CONNECTED, answerNav);
    expect(await screen.findByLabelText('Loading the conversation')).toBeTruthy();
    show(silentComputer(), ASLEEP, answerNav);
    expect(await screen.findByText(ASLEEP_WORDS)).toBeTruthy();
  });

  it('a Code conversation shows the code layout: the note, the files changed and the commands with their output, and nothing to approve', async () => {
    const link = connectedComputer({ load_conversation: CODE_CONVERSATION, agent_start: 'session-c', agent_send: 'run-c' });
    show(link, CONNECTED, codeNav);
    const note = await screen.findByRole('note');
    expect(note.textContent).toBe('Code changes are approved on your computer.');
    const question = screen.getByRole('heading', { level: 1, name: 'Make the reranker skip empty passages' });
    expect(question.classList.contains('mono')).toBe(true);
    expect(document.querySelector('[data-layout="code"]')?.classList.contains('layout-code')).toBe(true);

    const files = screen.getByRole('region', { name: 'Files changed' });
    expect(within(files).getByText('2 files changed')).toBeTruthy();
    expect(within(files).getByText('src/rerank.rs')).toBeTruthy();
    expect(within(files).getByText('2 edits')).toBeTruthy();
    expect(within(files).getByText('tests/rerank_empty.rs')).toBeTruthy();
    expect(within(files).getByText('Written')).toBeTruthy();
    expect(within(files).queryByText('Cargo.toml')).toBeNull();

    const commands = screen.getByRole('region', { name: 'Commands run' });
    expect(within(commands).getByText('cargo test --test rerank_empty')).toBeTruthy();
    expect(within(commands).getByText('Exit 0')).toBeTruthy();
    expect(within(commands).getByLabelText('Output of cargo test --test rerank_empty').textContent).toBe('test result: ok. 3 passed; 0 failed');
    expect(within(commands).getByText('Failed, exit 101')).toBeTruthy();
    expect(within(commands).getByLabelText('Output of cargo clippy -- -D warnings').textContent).toBe('…\nerror: unused variable `x`');

    // Nothing in a Code conversation approves a step on the phone.
    expect(screen.queryByRole('button', { name: /^(Allow|Approve|Review)\b/ })).toBeNull();

    // A follow-up question is asked, in a session that is not Code (the computer refuses Code from a phone).
    expect((screen.getByLabelText('Ask a follow-up') as HTMLInputElement).placeholder).toBe('Ask a question about this code');
    fireEvent.change(screen.getByLabelText('Ask a follow-up'), { target: { value: 'Why skip them?' } });
    fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(link.calls.some(c => c.command === 'agent_start')).toBe(true));
    const start = link.calls.find(c => c.command === 'agent_start');
    expect(start?.args).toEqual({ conversationId: 'code-1', workspaceId: 'w1' });
    expect(start?.args).not.toHaveProperty('mode');
  });

  it('a Creator conversation says where to see its preview, with what it made', async () => {
    const creator = {
      ...CODE_CONVERSATION,
      id: 'make-1',
      title: 'A poster of the results',
      agentMode: 'creator',
      messages: [
        { id: 'q', role: 'user', content: 'A poster of the results', timestamp: ago(40) },
        {
          id: 'a',
          role: 'assistant',
          content: 'The poster is ready.',
          timestamp: ago(40),
          transcript: {
            runId: 'r',
            status: 'completed',
            blocks: [{ kind: 'text', id: 'm', text: 'The poster is ready.' }],
            steps: {
              w: { tool: 'write', status: 'done', args: { path: 'index.html' } },
              f: { tool: 'finish_creation', status: 'done', detail: { summary: 'A one-page poster.', choices: [] } },
            },
            passages: [],
            groundings: [],
          },
        },
      ],
    };
    show(connectedComputer({ load_conversation: creator }), CONNECTED, { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'answer', conversationId: 'make-1', question: null, workspaceId: null }] });
    expect(await screen.findByText('Open on your computer to see the preview')).toBeTruthy();
    expect(screen.getByText('A one-page poster.')).toBeTruthy();
    expect(screen.getByText('1 file made')).toBeTruthy();
    expect(screen.queryByRole('note')).toBeNull();
    // Without a finish summary or files, the plain line alone.
    show(connectedComputer({ load_conversation: { ...creator, messages: creator.messages.slice(0, 1) } }), CONNECTED, { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'answer', conversationId: 'make-1', question: null, workspaceId: null }] });
    expect(await screen.findByText('Open on your computer to see the preview')).toBeTruthy();
    expect(screen.queryByText(/file made/)).toBeNull();
  });

  it('a conversation without a mode uses the reading layout', async () => {
    show(connectedComputer(), CONNECTED, answerNav);
    await screen.findByText('Retrieve token.');
    expect(document.querySelector('[data-layout="research"]')).toBeTruthy();
    expect(screen.queryByRole('note')).toBeNull();
    expect(screen.queryByText('Open on your computer to see the preview')).toBeNull();
    expect(screen.getByRole('heading', { level: 1, name: 'How does Self-RAG decide when to retrieve?' }).classList.contains('serif')).toBe(true);
  });

  it('says when the conversation is gone', async () => {
    show(connectedComputer({ load_conversation: null }), CONNECTED, answerNav);
    expect(await screen.findByText('This conversation is no longer on your computer.')).toBeTruthy();
  });
});

describe('Conversations', () => {
  it('lists them by when, pinned first, and searches titles', async () => {
    show(connectedComputer(), CONNECTED, conversationsNav);
    expect(await screen.findByText('Pinned')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Search conversations'), { target: { value: 'self-rag' } });
    expect(screen.queryByText('Where should the best passages go?')).toBeNull();
    expect(screen.getByText('How does Self-RAG decide when to retrieve?')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Search conversations'), { target: { value: 'zebra' } });
    expect(screen.getByText('No conversation title has "zebra".')).toBeTruthy();
  });

  it('shows each row with its kind, title, workspace and time, and filters by kind', async () => {
    show(connectedComputer(), CONNECTED, conversationsNav);
    const row = (title: string) => screen.getByText(title).closest('button') as HTMLElement;
    await screen.findByText('Make the reranker skip empty passages');
    // The mode icon is left of the title, named for its kind; a record without a mode is Research.
    const kindOf = (title: string) => row(title).querySelector('.mode-icon')?.getAttribute('aria-label');
    expect(kindOf('How does Self-RAG decide when to retrieve?')).toBe('Research');
    expect(kindOf('Where should the best passages go?')).toBe('Research');
    expect(kindOf('Make the reranker skip empty passages')).toBe('Code');
    expect(kindOf('A poster of the results')).toBe('Creator');
    const first = row('Make the reranker skip empty passages').firstElementChild;
    expect(first?.classList.contains('mode-code')).toBe(true);
    expect(row('How does Self-RAG decide when to retrieve?').querySelector('.mode-research')).toBeTruthy();
    expect(row('A poster of the results').querySelector('.mode-creator')).toBeTruthy();
    // The workspace by name, "No workspace" without one, and no name for one not listed.
    expect(await within(row('How does Self-RAG decide when to retrieve?')).findByText(/^Vector DB reading · \d+ minutes ago$/)).toBeTruthy();
    expect(within(row('Where should the best passages go?')).getByText(/^No workspace · /)).toBeTruthy();
    expect(within(row('A poster of the results')).getByText(/^\d+ minutes ago$/)).toBeTruthy();

    // The chips: All is chosen; Code shows only Code.
    const chips = within(screen.getByRole('group', { name: 'Show conversations' })).getAllByRole('button');
    expect(chips.map(c => c.textContent)).toEqual(['All', 'Research', 'Code', 'Creator']);
    expect(screen.getByRole('button', { name: 'All' }).getAttribute('aria-pressed')).toBe('true');
    fireEvent.click(screen.getByRole('button', { name: 'Code' }));
    expect(screen.getByRole('button', { name: 'Code' }).getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByText('Make the reranker skip empty passages')).toBeTruthy();
    expect(screen.queryByText('How does Self-RAG decide when to retrieve?')).toBeNull();
    expect(screen.queryByText('A poster of the results')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Research' }));
    expect(screen.getByText('How does Self-RAG decide when to retrieve?')).toBeTruthy();
    expect(screen.getByText('Where should the best passages go?')).toBeTruthy();
    expect(screen.queryByText('Make the reranker skip empty passages')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Creator' }));
    expect(screen.getByText('A poster of the results')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Search conversations'), { target: { value: 'reranker' } });
    expect(screen.getByText('No Creator conversation title has "reranker".')).toBeTruthy();
  });

  it('loading, empty and asleep', async () => {
    show(silentComputer(), CONNECTED, conversationsNav);
    expect(await screen.findByLabelText('Loading your conversations')).toBeTruthy();
    show(emptyComputer(), CONNECTED, conversationsNav);
    expect(await screen.findByText('No conversations yet. Ask a question to start one.')).toBeTruthy();
    show(silentComputer(), ASLEEP, conversationsNav);
    expect(await screen.findByText(ASLEEP_WORDS)).toBeTruthy();
  });
});

describe('Library', () => {
  it('the_tree_matches_the_desktop_at_three_levels: folders at any depth with breadcrumbs, files as their first page', async () => {
    const link = connectedComputer();
    show(link, CONNECTED, initialNav('library'));
    fireEvent.click(await screen.findByRole('button', { name: /Papers/ }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Papers' })).toBeTruthy();
    expect(await screen.findByAltText('self-rag.pdf, page 1')).toBeTruthy();
    // A file without pages shows its name and type.
    expect(screen.getByText('md')).toBeTruthy();
    expect(link.calls.find(c => c.command === 'read_source_page')?.args).toEqual({ filePath: '/papers/self-rag.pdf', page: 1, width: 320 });
    fireEvent.click(await screen.findByRole('button', { name: /Retrieval/ }));
    const crumbs = await screen.findByRole('navigation', { name: 'Folder path' });
    await waitFor(() => expect(within(crumbs).getAllByRole('button').map(b => b.textContent)).toEqual(['Library', 'Papers', 'Retrieval']));
    fireEvent.click(within(crumbs).getByRole('button', { name: 'Library' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Library' })).toBeTruthy();
  });

  it('opens a file in the Reader', async () => {
    show(connectedComputer(), CONNECTED, folderNav);
    fireEvent.click(await screen.findByRole('button', { name: 'Open notes.md' }));
    const reader = await screen.findByRole('dialog', { name: 'notes.md' });
    expect(await within(reader).findByText('Retrieve less, but better.')).toBeTruthy();
    expect(within(reader).getByRole('button', { name: 'Papers' })).toBeTruthy();
  });

  it('loading, empty and asleep', async () => {
    show(silentComputer(), CONNECTED, initialNav('library'));
    expect(await screen.findByLabelText('Loading the folder')).toBeTruthy();
    show(emptyComputer(), CONNECTED, initialNav('library'));
    expect(await screen.findByText('No folders linked yet')).toBeTruthy();
    show(silentComputer(), ASLEEP, initialNav('library'));
    expect(await screen.findByText(ASLEEP_WORDS)).toBeTruthy();
  });
});

describe('the Reader', () => {
  const readerNav = (path: string, name: string): Nav => ({
    ...folderNav,
    reader: { path, name, page: null, passage: null, back: 'Papers', origin: null },
  });
  const kind = (kind: string, extension: string) => (args: Record<string, unknown>) => ({
    path: args.filePath,
    fileName: String(args.filePath).split('/').pop(),
    extension,
    sizeBytes: 10,
    kind,
  });

  it('each_file_kind_opens_in_the_reader', async () => {
    // A PDF: its pages, rendered on the computer at the phone's width.
    show(connectedComputer({ get_source_file_info: kind('pdf', 'pdf') }), CONNECTED, readerNav('/papers/a.pdf', 'a.pdf'));
    let reader = await screen.findByRole('dialog', { name: 'a.pdf' });
    expect(await within(reader).findByAltText('Page 1')).toBeTruthy();
    expect(within(reader).getByText('12 pages')).toBeTruthy();
    // An image: full width, zoomable.
    show(connectedComputer({ get_source_file_info: kind('image', 'png') }), CONNECTED, readerNav('/papers/board.png', 'board.png'));
    reader = await screen.findByRole('dialog', { name: 'board.png' });
    expect((await within(reader).findByAltText('board.png')).className).toBe('zoomable');
    // Text, Markdown, code and Office files: their extracted text.
    show(connectedComputer(), CONNECTED, readerNav('/papers/notes.md', 'notes.md'));
    reader = await screen.findByRole('dialog', { name: 'notes.md' });
    expect(await within(reader).findByText('Retrieve less, but better.')).toBeTruthy();
    // A spreadsheet: its sheets.
    show(
      connectedComputer({
        get_source_file_info: kind('table', 'xlsx'),
        read_source_table: [{ name: 'Results', headers: ['Model', 'Recall'], rows: [['HNSW', '0.97']], totalRows: 1, truncated: false }],
      }),
      CONNECTED,
      readerNav('/papers/results.xlsx', 'results.xlsx'),
    );
    reader = await screen.findByRole('dialog', { name: 'results.xlsx' });
    expect(await within(reader).findByRole('cell', { name: 'HNSW' })).toBeTruthy();
    // Anything else says where to open it.
    show(connectedComputer({ get_source_file_info: kind('unsupported', 'bin') }), CONNECTED, readerNav('/papers/a.bin', 'a.bin'));
    reader = await screen.findByRole('dialog', { name: 'a.bin' });
    expect(await within(reader).findByText(/cannot be shown on the phone/)).toBeTruthy();
  });

  it('loading and asleep', async () => {
    show(silentComputer(), CONNECTED, readerNav('/papers/a.pdf', 'a.pdf'));
    expect(await screen.findByLabelText('Loading')).toBeTruthy();
    show(silentComputer(), ASLEEP, readerNav('/papers/a.pdf', 'a.pdf'));
    const reader = await screen.findByRole('dialog', { name: 'a.pdf' });
    expect(await within(reader).findByText(ASLEEP_WORDS)).toBeTruthy();
  });
});

describe('Settings', () => {
  it('says where the computer is, and offers to try again while it is asleep', async () => {
    show(connectedComputer(), CONNECTED, initialNav('settings'));
    expect(await screen.findByText('Your computer is connected.')).toBeTruthy();
    show(silentComputer(), ASLEEP, initialNav('settings'));
    expect(await screen.findByText('Your computer is asleep or offline. Ask when it is back.')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(source.reconnects).toBe(1));
  });
});

describe('Approvals', () => {
  it('lists what waits; a step the phone may allow opens the sheet, the rest say where to approve', async () => {
    const link = connectedComputer();
    show(link, CONNECTED, initialNav('approvals'));
    expect(await screen.findByText('Save a note about Self-RAG')).toBeTruthy();
    expect(screen.getByText('Approve this on your computer')).toBeTruthy();
    expect(screen.getAllByRole('button', { name: 'Review' })).toHaveLength(1);
    // The approvals' sessions are subscribed, so they stay answerable.
    await waitFor(() => expect(link.subscriptions.at(-1)).toEqual(['s1', 's2']));
    fireEvent.click(screen.getByRole('button', { name: 'Review' }));
    const sheet = await screen.findByRole('dialog', { name: 'Approve a step' });
    fireEvent.click(within(sheet).getByRole('button', { name: 'Allow' }));
    await waitFor(() => expect(link.calls.find(c => c.command === 'agent_approve')?.args).toEqual({ sessionId: 's1', stepId: 'note-1', approved: true }));
    expect(await screen.findByText('Allowed. Your computer goes on.')).toBeTruthy();
  });

  it('a refusal from the computer is shown in its words', async () => {
    show(connectedComputer({ agent_approve: () => Promise.reject({ code: 'refused', message: 'Approve this on your computer' }) }), CONNECTED, {
      ...initialNav('approvals'),
      sheet: { sessionId: 's1', stepId: 'note-1' },
    });
    const sheet = await screen.findByRole('dialog', { name: 'Approve a step' });
    await within(sheet).findByText('Save a note about Self-RAG');
    fireEvent.click(within(sheet).getByRole('button', { name: 'Allow' }));
    // The note says it in the computer's words, beside the Code step's own line.
    await waitFor(() => expect(screen.getAllByText('Approve this on your computer')).toHaveLength(2));
  });

  it('loading, empty and asleep', async () => {
    show(silentComputer(), CONNECTED, initialNav('approvals'));
    expect(await screen.findByLabelText('Loading approvals')).toBeTruthy();
    show(emptyComputer(), CONNECTED, initialNav('approvals'));
    expect(await screen.findByText('Nothing needs you right now. When the assistant wants a yes, it asks here.')).toBeTruthy();
    show(silentComputer(), ASLEEP, initialNav('approvals'));
    expect(await screen.findByText(ASLEEP_WORDS)).toBeTruthy();
  });
});

describe('Workspaces and the sections still to come', () => {
  it('lists the workspaces, and says the rest is coming without showing any data', async () => {
    show(connectedComputer(), CONNECTED, initialNav('workspaces'));
    expect(await screen.findByText('Vector DB reading')).toBeTruthy();
    show(emptyComputer(), CONNECTED, initialNav('workspaces'));
    expect(await screen.findByText('No workspaces yet')).toBeTruthy();
    show(silentComputer(), ASLEEP, initialNav('workspaces'));
    expect(await screen.findByText(ASLEEP_WORDS)).toBeTruthy();
    for (const section of ['tasks', 'activity', 'artifacts', 'connectors'] as const) {
      show(connectedComputer(), CONNECTED, initialNav(section));
      expect(await screen.findByText('Coming to the phone soon')).toBeTruthy();
    }
  });
});

describe('not paired', () => {
  it('opens at pairing, and a screen asked for while unpaired offers to pair', async () => {
    source = fakeSource(silentComputer(), { state: 'unpaired' });
    const { unmount } = render(<App source={source} />);
    expect(await screen.findByRole('heading', { name: 'Pair with your computer' })).toBeTruthy();
    const readies = source.readies;
    await waitFor(() => expect(readies).toEqual([['pair', 'dark']]));
    unmount();
    show(silentComputer(), { state: 'unpaired' }, initialNav('library'));
    fireEvent.click(await screen.findByRole('button', { name: 'Pair this phone' }));
    expect(await screen.findByRole('heading', { name: 'Pair with your computer' })).toBeTruthy();
  });
});

describe('the theme', () => {
  it('follows the phone, live, until the user picks Light or Dark', async () => {
    show(connectedComputer(), CONNECTED, initialNav('settings'));
    await screen.findByText('Theme');
    expect(document.documentElement.dataset.theme).toBe('dark');
    await waitFor(() => expect(source.readies).toEqual([['test', 'dark']]));
    act(() => setPhoneTheme('light'));
    expect(document.documentElement.dataset.theme).toBe('light');
    fireEvent.click(screen.getByRole('radio', { name: 'Dark' }));
    expect(document.documentElement.dataset.theme).toBe('dark');
    act(() => setPhoneTheme('light'));
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(window.localStorage.getItem('shodh.theme')).toBe('dark');
    fireEvent.click(screen.getByRole('radio', { name: 'Phone' }));
    expect(document.documentElement.dataset.theme).toBe('light');
    expect(window.localStorage.getItem('shodh.theme')).toBeNull();
  });

  it('the menu has the same choice', async () => {
    show(connectedComputer(), CONNECTED, { ...initialNav('ask'), drawer: true });
    const menu = await screen.findByRole('navigation', { name: 'All sections' });
    fireEvent.click(within(menu).getByRole('radio', { name: 'Light' }));
    expect(document.documentElement.dataset.theme).toBe('light');
  });
});

describe('the menu', () => {
  it("lists the desktop's sections in its order and takes the app to each", async () => {
    show(connectedComputer(), CONNECTED, initialNav('ask'));
    fireEvent.click(await screen.findByRole('button', { name: 'Open menu' }));
    const menu = await screen.findByRole('navigation', { name: 'All sections' });
    const items = within(menu)
      .getAllByRole('button')
      .map(b => b.textContent?.replace(/\d+ ?$/, '').replace(/\d+$/, '') ?? '');
    expect(items).toEqual(['Ask', 'Artifacts', 'Library', 'Workspaces', 'Connectors', 'Tasks', 'Approvals', 'Activity', 'Settings']);
    // The approvals waiting are counted.
    expect(await within(menu).findByLabelText('2 waiting')).toBeTruthy();
    fireEvent.click(within(menu).getByRole('button', { name: /^Library/ }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Library' })).toBeTruthy();
    await waitFor(() => expect(screen.queryByRole('navigation', { name: 'All sections' })).toBeNull());
    // The tab bar follows.
    const tabs = screen.getByRole('navigation', { name: 'Main' });
    expect(within(tabs).getByRole('button', { name: 'Library' }).getAttribute('aria-current')).toBe('page');
    fireEvent.click(screen.getByRole('button', { name: 'Open menu' }));
    fireEvent.click(within(await screen.findByRole('navigation', { name: 'All sections' })).getByRole('button', { name: /^Tasks/ }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Tasks' })).toBeTruthy();
    fireEvent.click(within(screen.getByRole('navigation', { name: 'Main' })).getByRole('button', { name: /Approvals/ }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Approvals' })).toBeTruthy();
  });
});

// ── Every control is at least 44px by 44px ──────────────────────────

const stylesheet = readFileSync(resolve(process.cwd(), 'src/theme.css'), 'utf8');
const desktopCss = readFileSync(resolve(process.cwd(), '../app/src/index.css'), 'utf8');

function px(value: string): number | null {
  const match = /^(\d+(?:\.\d+)?)px$/.exec(value.trim());
  return match ? Number(match[1]) : null;
}

function checkTargets(where: string) {
  const controls = document.querySelectorAll<HTMLElement>('button, a[href], input, textarea, select, [role="button"], [role="radio"]');
  expect(controls.length, where).toBeGreaterThan(0);
  for (const control of controls) {
    const name = `${where}: ${control.tagName.toLowerCase()}.${control.className || '(no class)'} "${control.textContent?.trim().slice(0, 30) ?? ''}"`;
    // A citation chip's 44px is its ::before (tests/rules.test.ts).
    if (control.classList.contains('chip')) continue;
    const style = getComputedStyle(control);
    const height = Math.max(px(style.minHeight) ?? 0, px(style.height) ?? 0);
    expect(height, `${name} is at least 44px tall`).toBeGreaterThanOrEqual(44);
    if (control.tagName === 'INPUT' || control.tagName === 'TEXTAREA') continue;
    const width = Math.max(px(style.minWidth) ?? 0, px(style.width) ?? 0);
    expect(width, `${name} is at least 44px wide`).toBeGreaterThanOrEqual(44);
  }
}

describe('every control', () => {
  beforeEach(() => {
    const style = document.createElement('style');
    style.id = 'phone-css';
    style.textContent = `${tokensCss(desktopCss)}\n${stylesheet}`;
    document.head.appendChild(style);
  });
  afterEach(() => document.getElementById('phone-css')?.remove());

  it('is 44px or more on every screen', async () => {
    const screens: Array<[string, Nav, LinkStatus]> = [
      ['home', initialNav('ask'), CONNECTED],
      ['answer', answerNav, CONNECTED],
      ['conversations', conversationsNav, CONNECTED],
      ['conversations filtered', { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'conversations', filter: 'code' }] }, CONNECTED],
      ['code conversation', codeNav, CONNECTED],
      ['library', folderNav, CONNECTED],
      ['approvals', initialNav('approvals'), CONNECTED],
      ['approval sheet', { ...initialNav('approvals'), sheet: { sessionId: 's1', stepId: 'note-1' } }, CONNECTED],
      ['settings', initialNav('settings'), CONNECTED],
      ['workspaces', initialNav('workspaces'), CONNECTED],
      ['tasks', initialNav('tasks'), CONNECTED],
      ['menu', { ...initialNav('ask'), drawer: true }, CONNECTED],
      ['reader', { ...folderNav, reader: { path: '/papers/notes.md', name: 'notes.md', page: null, passage: 'Cited.', back: 'Papers', origin: null } }, CONNECTED],
      ['home asleep', initialNav('ask'), ASLEEP],
      ['library unpaired', initialNav('library'), { state: 'unpaired' }],
    ];
    for (const [name, nav, status] of screens) {
      show(connectedComputer(), status, nav);
      // Let the screen read its data.
      await act(async () => {
        await new Promise(resolve => setTimeout(resolve, 0));
      });
      checkTargets(name);
    }
    shown?.unmount();
    shown = null;
    source = fakeSource(silentComputer(), { state: 'unpaired' });
    const { unmount } = render(<App source={source} />);
    await screen.findByRole('heading', { name: 'Pair with your computer' });
    checkTargets('pair');
    unmount();
  });
});
