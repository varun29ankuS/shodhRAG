/**
 * How the assistant's moves reach the screen (lib/navigationGate): bursts fold into
 * one move, nothing moves while the user types, and a navigation's focus opens the
 * connector page it names. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/navigationGate.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  COALESCE_MS,
  SWITCH_GAP_MS,
  TYPING_QUIET_MS,
  arrive,
  createTypingTracker,
  dropHeld,
  flush,
  goThere,
  heldNavigationText,
  holdsDraft,
  initialCoalescer,
  initialGate,
  isEditable,
  offerNavigation,
  viewOpened,
} from '../src/lib/navigationGate.ts';
import { announce, subscribeAnnouncements } from '../src/lib/announcer.ts';
import { connectorFocus } from '../src/features/connectors/navigation.ts';

const emptyComposer = { tagName: 'TEXTAREA', value: '' };
const draft = { tagName: 'TEXTAREA', value: 'Can you also check the ' };
const search = { tagName: 'INPUT', type: 'search', value: '' };
const button = { tagName: 'BUTTON' };

interface Nav {
  view: string | null;
  target: string;
}

const tasks = (n: number): Nav => ({ view: 'tasks', target: `task:${n}` });
const library: Nav = { view: 'library', target: 'source:s1' };

test('text fields count as typing places; buttons, checkboxes and read-only fields do not', () => {
  assert.equal(isEditable(emptyComposer), true);
  assert.equal(isEditable(search), true);
  assert.equal(isEditable({ tagName: 'input' }), true, 'an input without a type takes text');
  assert.equal(isEditable({ tagName: 'DIV', isContentEditable: true }), true);
  assert.equal(isEditable(button), false);
  assert.equal(isEditable({ tagName: 'INPUT', type: 'checkbox' }), false);
  assert.equal(isEditable({ tagName: 'INPUT', type: 'range' }), false);
  assert.equal(isEditable({ tagName: 'TEXTAREA', readOnly: true }), false);
  assert.equal(isEditable({ tagName: 'INPUT', type: 'text', disabled: true }), false);
  assert.equal(isEditable(null), false);
  assert.equal(holdsDraft(draft), true);
  assert.equal(holdsDraft({ tagName: 'TEXTAREA', value: '   ' }), false, 'spaces are no draft');
  assert.equal(holdsDraft({ tagName: 'DIV', isContentEditable: true, textContent: 'note' }), true);
  assert.equal(holdsDraft({ tagName: 'BUTTON', value: 'Send' }), false);
});

test("don't yank while typing: a draft, dictation or recent input holds; an empty composer after send does not", () => {
  const tracker = createTypingTracker();
  // A half-written message, the user pausing to think long past the quiet time: typing.
  assert.equal(tracker.isTyping(draft, 60_000), true);
  // The composer keeps the focus after a message is sent: empty and untouched, not typing.
  assert.equal(tracker.isTyping(emptyComposer, 1_000), false);
  // Typed into an empty field moments ago (a search box being cleared and retyped): typing.
  tracker.note(search, 1_000);
  assert.equal(tracker.isTyping(search, 1_000 + TYPING_QUIET_MS - 1), true);
  assert.equal(tracker.isTyping(search, 1_000 + TYPING_QUIET_MS), false, 'quiet again');
  // Sending resets: the Enter that sent the message does not hold the moves that follow.
  tracker.note(emptyComposer, 5_000);
  assert.equal(tracker.isTyping(emptyComposer, 5_100), true);
  tracker.reset();
  assert.equal(tracker.isTyping(emptyComposer, 5_100), false);
  // Dictation writes the composer through React state (no input event) with the focus on
  // the Dictate button: still typing while it listens.
  tracker.setDictating(true);
  assert.equal(tracker.isTyping(button, 9_000), true);
  tracker.setDictating(false);
  assert.equal(tracker.isTyping(button, 9_000), false);
  // Typed, then clicked a button: the focus left the field.
  tracker.note(draft, 10_000);
  assert.equal(tracker.isTyping(button, 10_100), false);
  // Input events from things that are not text fields are not typing.
  const other = createTypingTracker();
  other.note({ tagName: 'INPUT', type: 'range' }, 1_000);
  assert.equal(other.isTyping(search, 1_100), false);
});

test('a burst of moves becomes one move to the last target, counted', () => {
  let state = initialCoalescer<Nav>();
  // "Plan my week": the first task goes at once.
  let result = arrive(state, tasks(1), 'tasks', 0);
  assert.deepEqual(result.emit, { nav: tasks(1), count: 1 });
  state = result.state;
  // Seven more, each within the debounce of the last, wait; the last one wins.
  for (let i = 2; i <= 8; i++) {
    result = arrive(state, tasks(i), 'tasks', (i - 1) * 500);
    assert.equal(result.emit, null, `move ${i} waits`);
    state = result.state;
  }
  const dueAt = state.dueAt ?? Number.NaN;
  assert.equal(flush(state, dueAt - 1).emit, null, 'not before it is due');
  const flushed = flush(state, dueAt);
  assert.deepEqual(flushed.emit, { nav: tasks(8), count: 7 });
  state = flushed.state;
  assert.equal(state.pending, null);
  // A quiet spell later, the next move goes at once again.
  const later = arrive(state, tasks(9), 'tasks', (state.lastAt ?? 0) + COALESCE_MS);
  assert.deepEqual(later.emit, { nav: tasks(9), count: 1 });
});

test('the view switches at most once per SWITCH_GAP_MS', () => {
  let state = initialCoalescer<Nav>();
  state = arrive(state, tasks(1), 'tasks', 0).state;
  // A quiet moment later, a move to another view still waits for the gap.
  const early = arrive(state, library, 'library', COALESCE_MS + 10);
  assert.equal(early.emit, null);
  assert.equal(early.state.dueAt, SWITCH_GAP_MS);
  const due = flush(early.state, SWITCH_GAP_MS);
  assert.deepEqual(due.emit, { nav: library, count: 1 });
  // A move that stays on the current view (the System status panel) is not held back.
  const panel: Nav = { view: null, target: 'status:search' };
  const stays = arrive(due.state, panel, null, SWITCH_GAP_MS + COALESCE_MS);
  assert.deepEqual(stays.emit, { nav: panel, count: 1 });
});

test("don't yank while typing: the move is held, nothing is followed until Go there", () => {
  // Not typing: followed at once.
  const idle = offerNavigation(initialGate<Nav>(), { nav: library, count: 1 }, false);
  assert.deepEqual(idle.follow, library);
  assert.equal(idle.state.held, null);

  // Typing: held, nothing (view, item or conversation) is followed, and the chip appears.
  const typing = offerNavigation(initialGate<Nav>(), { nav: tasks(1), count: 1 }, true);
  assert.equal(typing.follow, null);
  assert.deepEqual(typing.state.held, { nav: tasks(1), count: 1 });
  assert.equal(typing.appeared, true, 'announced once');

  // More moves while still typing replace the held one and add up; the chip updates silently.
  const newer = offerNavigation(typing.state, { nav: tasks(8), count: 7 }, true);
  assert.equal(newer.follow, null);
  assert.deepEqual(newer.state.held, { nav: tasks(8), count: 8 });
  assert.equal(newer.appeared, false);

  // "Go there" follows exactly the held move, once.
  const go = goThere(newer.state);
  assert.deepEqual(go.follow, tasks(8));
  assert.equal(go.state.held, null);
  assert.equal(goThere(go.state).follow, null, 'nothing left to follow');

  // A move followed after typing stopped drops the stale chip.
  const after = offerNavigation(newer.state, { nav: library, count: 1 }, false);
  assert.deepEqual(after.follow, library);
  assert.equal(after.state.held, null);
});

test('the chip goes on a dismiss, a conversation switch, a new message, or opening its view; not when the run ends', () => {
  const held = offerNavigation(initialGate<Nav>(), { nav: tasks(1), count: 1 }, true).state;
  // Dismiss, a conversation switch and a new send all drop it without following.
  assert.equal(dropHeld(held).held, null);
  assert.equal(goThere(dropHeld(held)).follow, null);
  // The user opens Tasks themselves: spent. Opening another view keeps it.
  assert.equal(viewOpened(held, 'tasks').held, null);
  assert.deepEqual(viewOpened(held, 'library'), held);
  // The run ending is no event of the gate: the chip stays.
  assert.deepEqual(held.held, { nav: tasks(1), count: 1 });
});

test('the chip says what Shodh has for the user, and where', () => {
  assert.equal(heldNavigationText('Tasks', 'Renew lease', 1), 'Shodh has “Renew lease” to show you in Tasks');
  assert.equal(heldNavigationText('Library', null, 1), 'Shodh has something to show you in Library');
  assert.equal(heldNavigationText('Tasks', 'Renew lease', 8), 'Shodh has 8 things to show you in Tasks');
  assert.equal(heldNavigationText(null, 'System status', 1), 'Shodh has “System status” to show you');
});

test('announcements reach every listener once, and blank ones none', () => {
  const heard: string[] = [];
  const stop = subscribeAnnouncements(m => heard.push(m));
  announce('Shodh opened Tasks: Renew lease');
  announce('   ');
  stop();
  announce('after');
  assert.deepEqual(heard, ['Shodh opened Tasks: Renew lease']);
});

test('a navigation focus opens the connector page it names', () => {
  assert.deepEqual(connectorFocus('slack'), { connector: 'slack', tab: null });
  assert.deepEqual(connectorFocus('slack:indexing'), { connector: 'slack', tab: 'indexing' });
  assert.deepEqual(connectorFocus('telegram:people'), { connector: 'telegram', tab: 'people' });
  assert.deepEqual(connectorFocus('github:repositories:octo-org/hello'), {
    connector: 'github', tab: 'repositories', item: 'octo-org/hello',
  });
  assert.equal(connectorFocus('slack:billing'), null, 'no such tab');
  assert.equal(connectorFocus('telegram:indexing'), null, 'a Slack tab is not a Telegram tab');
  assert.equal(connectorFocus('teams'), null);
  assert.equal(connectorFocus('task-12'), null);
  assert.equal(connectorFocus(null), null);
});
