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
import { memoryDevice, type MemoryDevice } from './device';
import { queuedQuestion } from '../src/lib/outbox';

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

const pad2 = (n: number) => String(n).padStart(2, '0');
const todayKey = (() => {
  const d = new Date();
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
})();

const TASKS = [
  { id: 't1', title: 'Read the Self-RAG paper', status: 'pending', dueDate: todayKey, source: 'user', createdAt: ago(60), updatedAt: ago(60) },
  { id: 't2', title: 'Send the slides', status: 'completed', source: 'user', createdAt: ago(90), updatedAt: ago(30) },
];

const EVENTS = [
  { id: 'e1', title: 'Reading group', startTime: `${todayKey}T23:00`, endTime: `${todayKey}T23:30`, allDay: false, source: 'user', createdAt: ago(100) },
];

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
    load_tasks: TASKS,
    load_events: EVENTS,
    update_task: (args: Record<string, unknown>) => ({ ...TASKS[0], ...args }),
    ...overrides,
  });
}

function emptyComputer(): FakeLink {
  return connectedComputer({ conversations_list: [], library_folders: { folders: [] }, approvals_pending: [], workspaces_list: [] });
}

let source: FakeSource;
let device: MemoryDevice;
let shown: { unmount: () => void } | null = null;

/** Show the app at `nav` (the screen shown before goes). */
function show(link: FakeLink, status: LinkStatus, nav: Nav, phone: MemoryDevice = memoryDevice()) {
  shown?.unmount();
  source = fakeSource(link, status);
  device = phone;
  const result = render(<App source={source} start={{ screen: 'test', nav }} device={device} />);
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
    expect(await screen.findByText(/Reading group$/)).toBeTruthy();
    expect(screen.getByText('1 on your calendar today, and 1 task due.')).toBeTruthy();
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

  it('with the computer asleep says so, offers to try again, and keeps a question until it wakes', async () => {
    show(silentComputer(), ASLEEP, initialNav('ask'));
    expect(await screen.findByText(ASLEEP_WORDS)).toBeTruthy();
    expect(screen.getByText('Your computer is asleep. What you ask is sent when it wakes.')).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(source.reconnects).toBe(1));

    fireEvent.change(screen.getByLabelText('Ask about your library'), { target: { value: 'What did Asha say about HNSW?' } });
    fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    // Kept on the phone, and its conversation shows it waiting.
    expect(await screen.findByText('Waiting for your computer')).toBeTruthy();
    expect(screen.getByText('What did Asha say about HNSW?')).toBeTruthy();
    expect(screen.getByText('Sent when your computer wakes')).toBeTruthy();
    expect(device.outbox.items.map(i => [i.work.kind, i.state])).toEqual([['question', 'waiting']]);
  });

  it('an unpaired or outdated phone keeps nothing it could never send', async () => {
    show(silentComputer(), { state: 'update', message: 'Update Shodh on your phone: your computer has version 0.0.11.', desktop: '0.0.11' }, initialNav('ask'));
    expect(await screen.findByText('Update Shodh on your phone')).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
    expect(device.outbox.items).toEqual([]);
  });
});

describe('One-tap update', () => {
  it('finds the matching app, and one tap installs it once verified', async () => {
    const phone = memoryDevice({ update: { state: 'ready', version: '0.0.11', size: 24 * 1_048_576 } });
    show(silentComputer(), { state: 'update', message: 'Update Shodh on your phone: your computer has version 0.0.11.', desktop: '0.0.11' }, initialNav('settings'), phone);
    const button = await screen.findByRole('button', { name: 'Update to 0.0.11 (24 MB)' });
    expect(screen.getByText(/Your computer runs Shodh 0.0.11, and this phone runs/)).toBeTruthy();
    fireEvent.click(button);
    expect(await screen.findByText('The update is checked. Android asks you to confirm it.')).toBeTruthy();
    expect(phone.installs).toBe(1);
  });

  it('says plainly when the update is not published or was refused', async () => {
    const refused = memoryDevice({
      update: { state: 'ready', version: '0.0.11', size: 1_048_576 },
      install: async () => {
        throw 'The downloaded app is not the one Shodh released: its checksum differs from the release’s. It was not installed.';
      },
    });
    show(silentComputer(), { state: 'update', message: '', desktop: '0.0.11' }, initialNav('settings'), refused);
    fireEvent.click(await screen.findByRole('button', { name: 'Update to 0.0.11 (1 MB)' }));
    expect(await screen.findByText(/It was not installed\./)).toBeTruthy();
    show(silentComputer(), { state: 'update', message: '', desktop: '0.0.11' }, initialNav('settings'));
    expect(await screen.findByText('Shodh 0.0.11 for Android is not published yet.')).toBeTruthy();
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

describe('Send to Shodh', () => {
  it('Add picks files and sends them to the workspace chosen, "From my phone" by default', async () => {
    const phone = memoryDevice();
    phone.files.picks = [{ path: '/cache/outgoing/1/scan.pdf', name: 'scan.pdf' }];
    show(connectedComputer(), CONNECTED, initialNav('library'), phone);
    fireEvent.click(await screen.findByRole('button', { name: 'Add' }));
    const sheet = await screen.findByRole('dialog', { name: 'Send to Shodh' });
    expect(within(sheet).getByText('scan.pdf')).toBeTruthy();
    expect(within(sheet).getByRole('radio', { name: /From my phone/ }).getAttribute('aria-checked')).toBe('true');
    fireEvent.click(await within(sheet).findByRole('radio', { name: /Vector DB reading/ }));
    fireEvent.click(within(sheet).getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(phone.outbox.items.length).toBe(1));
    expect(phone.outbox.items[0].work).toMatchObject({ kind: 'file', name: 'scan.pdf', workspaceId: 'w1' });
    // Sent at once while the computer is connected.
    await waitFor(() => expect(phone.files.sent.map(f => f.name)).toEqual(['scan.pdf']));
  });

  it('what another app shares opens the sheet; Cancel removes the copies', async () => {
    const phone = memoryDevice();
    phone.files.incoming = { kind: 'files', files: [{ path: '/cache/outgoing/2/notes.md', name: 'notes.md' }] };
    show(silentComputer(), ASLEEP, initialNav('ask'), phone);
    const sheet = await screen.findByRole('dialog', { name: 'Send to Shodh' });
    expect(within(sheet).getByRole('button', { name: 'Send when it wakes' })).toBeTruthy();
    fireEvent.click(within(sheet).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(phone.files.discarded).toEqual(['/cache/outgoing/2/notes.md']));
    expect(phone.outbox.items).toEqual([]);
  });

  it('the Library shows "From my phone" as a folder of its own', async () => {
    show(
      connectedComputer({ library_folders: { folders: [{ name: 'From my phone', path: '/data/From my phone' }, { name: 'Papers', path: '/papers' }] } }),
      CONNECTED,
      initialNav('library'),
    );
    expect(await screen.findByText('Files you sent from this phone')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Photo' })).toBeTruthy();
  });
});

describe('Tasks', () => {
  it("shows today's agenda and the tasks, and ticks one off on the computer", async () => {
    const link = connectedComputer();
    show(link, CONNECTED, initialNav('tasks'));
    expect(await screen.findByText('Reading group')).toBeTruthy();
    expect(screen.getByText('23:00 to 23:30')).toBeTruthy();
    expect(screen.getByText('Read the Self-RAG paper')).toBeTruthy();
    expect(screen.getByText('Due today')).toBeTruthy();
    expect(screen.getByText('1 done')).toBeTruthy();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Tick off “Read the Self-RAG paper”' }));
    await waitFor(() => expect(link.calls.find(c => c.command === 'update_task')?.args).toEqual({ id: 't1', status: 'completed' }));
  });

  it('while the computer is away shows the sealed copy and keeps a new task until it wakes', async () => {
    const phone = memoryDevice();
    await phone.sealed.write('agenda', { tasks: TASKS, events: EVENTS, savedAt: Date.now() - 60_000 });
    show(silentComputer(), ASLEEP, initialNav('tasks'), phone);
    expect(await screen.findByText(/^Your computer is away\. This is your agenda as of/)).toBeTruthy();
    expect(screen.getByText('Read the Self-RAG paper')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('The new task'), { target: { value: 'Call Asha' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Add' })[0]);
    await waitFor(() => expect(phone.outbox.items.length).toBe(1));
    const work = phone.outbox.items[0].work;
    expect(work).toMatchObject({ kind: 'edit', command: 'create_task', args: { title: 'Call Asha', sourceRef: `phone:${phone.outbox.items[0].id}` } });
    expect(await screen.findByText('Add the task “Call Asha”')).toBeTruthy();
  });

  it('a delete in the phone calendar asks before Shodh removes its event', async () => {
    const phone = memoryDevice();
    await phone.calendar.choose({ enabled: true, chosen: [], shodhCalendar: null, useWithShodh: false });
    await phone.sealed.write('calendar-sync', {
      links: [{ desktopId: 'e1', calendarId: 9, eventId: 500, origin: 'shodh', base: { title: 'Reading group', start: `${todayKey}T23:00`, end: `${todayKey}T23:30`, allDay: false, location: null, description: '' }, phoneChangedAt: null }],
      history: [],
      pending: [],
    });
    const link = connectedComputer({ delete_event: true });
    show(link, CONNECTED, initialNav('tasks'), phone);
    expect(await screen.findByText('“Reading group” was removed from your phone’s calendar. Remove it from Shodh too?')).toBeTruthy();
    expect(link.calls.some(c => c.command === 'delete_event')).toBe(false);
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(link.calls.find(c => c.command === 'delete_event')?.args).toEqual({ id: 'e1' }));
  });
});

describe('Point the camera and ask, Ask about this, and briefs', () => {
  const finished = (link: FakeLink, sessionId: string, runId: string, text: string) =>
    queueMicrotask(() => {
      link.send('agent_event', { sessionId, event: { type: 'text_delta', runId, messageId: 'm', delta: text } });
      link.send('agent_event', { sessionId, event: { type: 'run_finished', runId, status: 'completed', durationMs: 1, error: null } });
    });

  it('a_photo_is_not_indexed_unless_kept', async () => {
    const phone = memoryDevice();
    phone.files.photo = { path: '/cache/outgoing/1/Photo.jpg', name: 'Photo.jpg' };
    let link!: FakeLink;
    link = connectedComputer({
      load_conversation: null,
      agent_start: 'session-p',
      agent_send: () => {
        finished(link, 'session-p', 'run-p', 'Adaptive retrieval [1].');
        return 'run-p';
      },
      phone_photo_keep: { name: 'Photo.jpg', path: '/data/From my phone/Photo.jpg', added: true, reason: null, notice: null },
    });
    show(link, CONNECTED, initialNav('ask'), phone);
    fireEvent.click(await screen.findByRole('button', { name: 'Ask about a photo' }));
    const sheet = await screen.findByRole('dialog', { name: 'Ask about your photo' });
    expect(await within(sheet).findByText('Retrieval should be adaptive.')).toBeTruthy();
    fireEvent.change(within(sheet).getByLabelText('Your question about the photo'), { target: { value: 'What does it say?' } });
    fireEvent.click(within(sheet).getByRole('button', { name: 'Ask' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'What does it say?' })).toBeTruthy();
    expect(phone.files.photosSent.map(f => f.name)).toEqual(['Photo.jpg']);
    // Asked about, not added: nothing asks the computer to keep it.
    expect(link.calls.some(c => c.command === 'phone_photo_keep')).toBe(false);
    // "Your photo" is its source; Keep in library adds it.
    fireEvent.click(await screen.findByRole('button', { name: 'Your photo, the photo you took' }));
    const photoSheet = await screen.findByRole('dialog', { name: 'Your photo' });
    fireEvent.click(within(photoSheet).getByRole('button', { name: 'Keep in library' }));
    await waitFor(() => expect(link.calls.find(c => c.command === 'phone_photo_keep')?.args).toEqual({ photoId: 'p'.repeat(32) }));
    expect(await within(photoSheet).findByText('Added to your library, in From my phone.')).toBeTruthy();
  });

  it('text shared from another app opens Ask about this', async () => {
    const phone = memoryDevice();
    phone.files.incoming = { kind: 'text', text: 'https://arxiv.org/abs/2310.11511', subject: 'Self-RAG' };
    let link!: FakeLink;
    link = connectedComputer({
      load_conversation: null,
      agent_start: 'session-s',
      agent_send: () => {
        finished(link, 'session-s', 'run-s', 'It adds reflection tokens.');
        return 'run-s';
      },
    });
    show(link, CONNECTED, initialNav('ask'), phone);
    const sheet = await screen.findByRole('dialog', { name: 'Ask about this' });
    expect(within(sheet).getByText('A link you shared: Shodh may open it to answer.')).toBeTruthy();
    fireEvent.click(within(sheet).getByRole('button', { name: 'Ask' }));
    await waitFor(() => expect(link.calls.some(c => c.command === 'agent_send')).toBe(true));
    expect(link.calls.find(c => c.command === 'agent_send')?.args).toMatchObject({
      text: 'What does my library say about this?\n\nhttps://arxiv.org/abs/2310.11511',
      textOrigin: 'typed',
    });
  });

  it('offers a brief only with calendar consent, and says when nothing matches', async () => {
    const link = connectedComputer({ meeting_brief: null });
    show(link, CONNECTED, initialNav('tasks'));
    expect(await screen.findByText('Reading group')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Brief' })).toBeNull();

    const phone = memoryDevice();
    phone.calendar.current = { enabled: true, chosen: [1], shodhCalendar: 9, useWithShodh: true };
    show(link, CONNECTED, initialNav('tasks'), phone);
    fireEvent.click(await screen.findByRole('button', { name: 'Brief' }));
    await waitFor(() => expect(link.calls.find(c => c.command === 'meeting_brief')?.args).toEqual({ eventId: 'e1' }));
    expect(await screen.findByText('Nothing in your library or your conversations is about this event.')).toBeTruthy();
  });
});

describe('Settings, Calendar', () => {
  it('sync is off until turned on; then the calendars to read and "Let Shodh use my calendar"', async () => {
    const phone = memoryDevice();
    show(connectedComputer(), CONNECTED, initialNav('settings'), phone);
    const sync = await screen.findByRole('switch', { name: "Sync with this phone's calendar" });
    expect(sync.getAttribute('aria-checked')).toBe('false');
    fireEvent.click(sync);
    fireEvent.click(await screen.findByRole('checkbox', { name: /Work/ }));
    await waitFor(() => expect(phone.calendar.current.chosen).toEqual([2]));
    expect(phone.calendar.current.shodhCalendar).toBe(9);
    expect(screen.getByText(/Off: events of the calendars you ticked show here only/)).toBeTruthy();
    fireEvent.click(screen.getByRole('switch', { name: 'Let Shodh use my calendar' }));
    await waitFor(() => expect(phone.calendar.current.useWithShodh).toBe(true));
  });

  it('says plainly when Android refuses the calendar', async () => {
    const phone = memoryDevice();
    phone.calendar.granted = false;
    show(connectedComputer(), CONNECTED, initialNav('settings'), phone);
    fireEvent.click(await screen.findByRole('switch', { name: "Sync with this phone's calendar" }));
    expect(await screen.findByText(/Shodh may not use this phone’s calendar/)).toBeTruthy();
    expect(phone.calendar.current.enabled).toBe(false);
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
    expect(await screen.findByText('Your computer is asleep. What you ask is sent when it wakes.')).toBeTruthy();
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
    for (const section of ['activity', 'artifacts', 'connectors'] as const) {
      show(connectedComputer(), CONNECTED, initialNav(section));
      expect(await screen.findByText('Coming to the phone soon')).toBeTruthy();
    }
  });
});

describe('not paired', () => {
  it('opens at pairing, and a screen asked for while unpaired offers to pair', async () => {
    source = fakeSource(silentComputer(), { state: 'unpaired' });
    const { unmount } = render(<App source={source} device={memoryDevice()} />);
    expect(await screen.findByRole('heading', { name: 'Pair with your computer' })).toBeTruthy();
    const readies = source.readies;
    await waitFor(() => expect(readies).toEqual([['pair', 'dark']]));
    unmount();
    show(silentComputer(), { state: 'unpaired' }, initialNav('library'));
    fireEvent.click(await screen.findByRole('button', { name: 'Pair this phone' }));
    expect(await screen.findByRole('heading', { name: 'Pair with your computer' })).toBeTruthy();
  });
});

describe('voice', () => {
  it('Talk puts what was said in the ask box; Read aloud names the sources from their metadata', async () => {
    const phone = memoryDevice();
    show(connectedComputer(), CONNECTED, initialNav('ask'), phone);
    fireEvent.click(await screen.findByRole('button', { name: 'Ask by speaking' }));
    await waitFor(() => expect((screen.getByLabelText('Ask about your library') as HTMLInputElement).value).toBe('What does Self-RAG retrieve?'));

    show(connectedComputer(), CONNECTED, answerNav, phone);
    fireEvent.click(await screen.findByRole('button', { name: 'Read aloud' }));
    await waitFor(() => expect(phone.voice.spoken).toHaveLength(1));
    expect(phone.voice.spoken[0]).toMatch(/^It decides itself\./);
    expect(phone.voice.spoken[0]).toMatch(/Source: self-rag, page 3\.$/);
    expect(phone.voice.spoken[0]).not.toContain('[1]');
  });
});

describe('Open in Shodh', () => {
  it("an event's link in the phone's calendar opens Tasks with that event", async () => {
    const phone = memoryDevice();
    phone.files.incoming = { kind: 'event', id: 'e1' };
    show(connectedComputer(), CONNECTED, initialNav('ask'), phone);
    expect(await screen.findByText(/^Opened from your phone’s calendar/)).toBeTruthy();
    expect(screen.getByRole('heading', { level: 1, name: 'Tasks' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Done' }));
    await waitFor(() => expect(screen.queryByText(/^Opened from your phone’s calendar/)).toBeNull());
  });
});

describe('the lock', () => {
  it('the_lock_is_required_on_open_by_default', async () => {
    const phone = memoryDevice();
    phone.lock.status_ = { required: true, hasPin: true, biometric: false, locked: true };
    source = fakeSource(connectedComputer(), CONNECTED);
    const { unmount } = render(<App source={source} device={phone} />);
    expect(await screen.findByRole('dialog', { name: 'Shodh is locked' })).toBeTruthy();
    // Nothing behind it runs: no conversation, nothing asked of the computer.
    expect(screen.queryByText('Continue')).toBeNull();
    fireEvent.change(screen.getByLabelText('Your PIN'), { target: { value: '000000' } });
    fireEvent.click(screen.getByRole('button', { name: 'Unlock' }));
    expect(await screen.findByText(/That is not your PIN/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Your PIN'), { target: { value: '482913' } });
    fireEvent.click(screen.getByRole('button', { name: 'Unlock' }));
    expect(await screen.findByText('Continue')).toBeTruthy();
    unmount();
  });

  it('the first time, the PIN is set before anything opens', async () => {
    const phone = memoryDevice();
    phone.lock.status_ = { required: true, hasPin: false, biometric: false, locked: true };
    source = fakeSource(connectedComputer(), CONNECTED);
    const { unmount } = render(<App source={source} device={phone} />);
    expect(await screen.findByRole('dialog', { name: 'Set a PIN' })).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Your new PIN'), { target: { value: '2468' } });
    fireEvent.change(screen.getByLabelText('The new PIN again'), { target: { value: '2469' } });
    fireEvent.click(screen.getByRole('button', { name: 'Set the PIN' }));
    expect(await screen.findByText('The two PINs are not the same.')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Your new PIN'), { target: { value: '2468' } });
    fireEvent.change(screen.getByLabelText('The new PIN again'), { target: { value: '2468' } });
    fireEvent.click(screen.getByRole('button', { name: 'Set the PIN' }));
    expect(await screen.findByText('Continue')).toBeTruthy();
    expect(phone.lock.pin).toBe('2468');
    unmount();
  });

  it('an unpaired phone shows pairing, not the lock', async () => {
    const phone = memoryDevice();
    phone.lock.status_ = { required: true, hasPin: false, biometric: false, locked: true };
    source = fakeSource(silentComputer(), { state: 'unpaired' });
    const { unmount } = render(<App source={source} device={phone} />);
    expect(await screen.findByRole('heading', { name: 'Pair with your computer' })).toBeTruthy();
    unmount();
  });
});

describe('Settings, Notifications', () => {
  it('is off by default; Stay connected and Push are the user’s to choose', async () => {
    const phone = memoryDevice();
    show(connectedComputer(), CONNECTED, initialNav('settings'), phone);
    const group = await screen.findByRole('radiogroup', { name: 'Notifications' });
    await waitFor(() => expect(within(group).getByRole('radio', { name: 'Off' }).getAttribute('aria-checked')).toBe('true'));
    expect(phone.notifications.sets).toEqual([]);
    fireEvent.click(within(group).getByRole('radio', { name: 'Stay connected' }));
    await waitFor(() => expect(phone.notifications.sets).toEqual([['stay', undefined]]));
    fireEvent.click(within(group).getByRole('radio', { name: 'Push' }));
    await waitFor(() => expect(phone.notifications.sets.at(-1)).toEqual(['push', 'io.heckel.ntfy']));
    expect(screen.getByText('Every notification says only what waits, never what it is about.')).toBeTruthy();
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

  it("Android's system bars follow the theme, live: light icons on dark, dark icons on light", async () => {
    show(connectedComputer(), CONNECTED, initialNav('settings'));
    await screen.findByText('Theme');
    await waitFor(() => expect(device.bars).toEqual(['dark']));
    act(() => setPhoneTheme('light'));
    await waitFor(() => expect(device.bars).toEqual(['dark', 'light']));
    fireEvent.click(screen.getByRole('radio', { name: 'Dark' }));
    await waitFor(() => expect(device.bars).toEqual(['dark', 'light', 'dark']));
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
    const queued = memoryDevice();
    await queued.outbox.add('q1', queuedQuestion({ conversationId: 'c9', text: 'What did Asha say?', workspaceId: null }));
    const updating = memoryDevice({ update: { state: 'ready', version: '0.0.11', size: 1_048_576 } });
    const sharing = memoryDevice();
    sharing.files.incoming = { kind: 'files', files: [{ path: '/cache/outgoing/1/scan.pdf', name: 'scan.pdf' }] };
    const syncing = memoryDevice();
    syncing.calendar.current = { enabled: true, chosen: [1], shodhCalendar: 9, useWithShodh: true };
    const asking = memoryDevice();
    asking.files.incoming = { kind: 'text', text: 'The meeting moved to Friday.', subject: null };
    const screens: Array<[string, Nav, LinkStatus, MemoryDevice?]> = [
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
      ['home with questions waiting', initialNav('ask'), ASLEEP, queued],
      ['settings with an update', initialNav('settings'), { state: 'update', message: '', desktop: '0.0.11' }, updating],
      ['send sheet', initialNav('library'), CONNECTED, sharing],
      ['settings with calendar sync', initialNav('settings'), CONNECTED, syncing],
      ['tasks with calendar consent', initialNav('tasks'), CONNECTED, syncing],
      ['ask about this', initialNav('ask'), CONNECTED, asking],
    ];
    for (const [name, nav, status, phone] of screens) {
      show(connectedComputer(), status, nav, phone);
      // Let the screen read its data.
      await act(async () => {
        await new Promise(resolve => setTimeout(resolve, 0));
      });
      checkTargets(name);
    }
    shown?.unmount();
    shown = null;
    source = fakeSource(silentComputer(), { state: 'unpaired' });
    const { unmount } = render(<App source={source} device={memoryDevice()} />);
    await screen.findByRole('heading', { name: 'Pair with your computer' });
    checkTargets('pair');
    unmount();

    // The lock, both ways it shows.
    for (const [name, hasPin] of [['lock', true], ['set a PIN', false]] as const) {
      const locked = memoryDevice();
      locked.lock.status_ = { required: true, hasPin, biometric: true, locked: true };
      source = fakeSource(connectedComputer(), CONNECTED);
      const lock = render(<App source={source} device={locked} />);
      await screen.findByRole('dialog', { name: hasPin ? 'Shodh is locked' : 'Set a PIN' });
      checkTargets(name);
      lock.unmount();
    }

    // Asking about a photo, and a brief.
    const camera = memoryDevice();
    camera.files.photo = { path: '/cache/outgoing/1/Photo.jpg', name: 'Photo.jpg' };
    show(connectedComputer(), CONNECTED, initialNav('ask'), camera);
    fireEvent.click(await screen.findByRole('button', { name: 'Ask about a photo' }));
    await screen.findByRole('dialog', { name: 'Ask about your photo' });
    checkTargets('photo ask');
    const briefing = memoryDevice();
    briefing.calendar.current = { enabled: true, chosen: [1], shodhCalendar: 9, useWithShodh: true };
    show(connectedComputer({ meeting_brief: { eventId: 'e1', title: 'Reading group', conversations: [{ id: 'c1', title: 'Reading group notes', updatedAt: ago(60) }], sources: [{ n: 1, file: 'self-rag.pdf', path: '/papers/self-rag.pdf', page: '3', text: 'Reading group: Self-RAG.', kind: 'document' }] } }), CONNECTED, initialNav('tasks'), briefing);
    fireEvent.click(await screen.findByRole('button', { name: 'Brief' }));
    await screen.findByText('Reading group notes');
    checkTargets('brief');
    shown?.unmount();
    shown = null;
  });
});
