/**
 * The assistant never pulls the user away while they type (lib/navigationGate),
 * and a navigation's focus opens the connector page it names. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/navigationGate.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  TYPING_QUIET_MS,
  createTypingTracker,
  dismissHeld,
  goThere,
  heldNavigationText,
  initialGate,
  isEditable,
  offerNavigation,
} from '../src/lib/navigationGate.ts';
import { connectorFocus } from '../src/features/connectors/navigation.ts';

const composer = { tagName: 'TEXTAREA' };
const search = { tagName: 'INPUT', type: 'search' };
const button = { tagName: 'BUTTON' };

interface Nav {
  view: string;
  target: string | null;
}

test('text fields count as typing places; buttons, checkboxes and read-only fields do not', () => {
  assert.equal(isEditable(composer), true);
  assert.equal(isEditable(search), true);
  assert.equal(isEditable({ tagName: 'input' }), true, 'an input without a type takes text');
  assert.equal(isEditable({ tagName: 'DIV', isContentEditable: true }), true);
  assert.equal(isEditable(button), false);
  assert.equal(isEditable({ tagName: 'INPUT', type: 'checkbox' }), false);
  assert.equal(isEditable({ tagName: 'INPUT', type: 'range' }), false);
  assert.equal(isEditable({ tagName: 'TEXTAREA', readOnly: true }), false);
  assert.equal(isEditable({ tagName: 'INPUT', type: 'text', disabled: true }), false);
  assert.equal(isEditable(null), false);
});

test('typing is recent input in the focused text field, not focus alone', () => {
  const tracker = createTypingTracker();
  // The composer keeps the focus after a message is sent: nothing typed yet, not typing.
  assert.equal(tracker.isTyping(composer, 1_000), false);
  tracker.note(composer, 1_000);
  assert.equal(tracker.isTyping(composer, 1_000 + TYPING_QUIET_MS - 1), true);
  assert.equal(tracker.isTyping(composer, 1_000 + TYPING_QUIET_MS), false, 'quiet again');
  // Typed, then clicked a button: the focus left the field.
  tracker.note(composer, 5_000);
  assert.equal(tracker.isTyping(button, 5_100), false);
  // Input events from things that are not text fields are not typing.
  const other = createTypingTracker();
  other.note({ tagName: 'INPUT', type: 'range' }, 1_000);
  assert.equal(other.isTyping(search, 1_100), false);
});

test("don't yank while typing: the navigation is held, nothing is followed until Go there", () => {
  const library: Nav = { view: 'library', target: 'source:s1' };
  const tasks: Nav = { view: 'tasks', target: 'task:t1' };

  // Not typing: followed at once.
  const idle = offerNavigation(initialGate<Nav>(), library, false);
  assert.deepEqual(idle.follow, library);
  assert.equal(idle.state.held, null);

  // Typing: held, and nothing (view, item or conversation) is followed.
  const typing = offerNavigation(initialGate<Nav>(), library, true);
  assert.equal(typing.follow, null);
  assert.deepEqual(typing.state.held, library);

  // A newer navigation while still typing replaces the held one: the chip names where the
  // assistant went last.
  const newer = offerNavigation(typing.state, tasks, true);
  assert.equal(newer.follow, null);
  assert.deepEqual(newer.state.held, tasks);

  // "Go there" follows exactly the held navigation, once.
  const go = goThere(newer.state);
  assert.deepEqual(go.follow, tasks);
  assert.equal(go.state.held, null);
  assert.equal(goThere(go.state).follow, null, 'nothing left to follow');

  // Dismissed: dropped, nothing followed.
  assert.equal(dismissHeld(newer.state).held, null);
  assert.equal(goThere(dismissHeld(newer.state)).follow, null);

  // A navigation followed after typing stopped drops the stale chip.
  const after = offerNavigation(newer.state, library, false);
  assert.deepEqual(after.follow, library);
  assert.equal(after.state.held, null);
});

test('the chip says where the assistant went', () => {
  assert.equal(heldNavigationText('Library'), 'The assistant opened Library');
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
