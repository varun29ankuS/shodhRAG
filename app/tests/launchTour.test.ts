/**
 * The launch test's views tour (`launch_probe.rs`, `views_tour`): the views it opens are
 * the app's own, in the sidebar's order, and the main window opens each as the sidebar does.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/launchTour.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { VIEW_TABS } from '../src/lib/viewTabs.ts';

const probe = readFileSync(new URL('../src-tauri/src/launch_probe.rs', import.meta.url), 'utf8');
const host = readFileSync(new URL('../src/features/launch/LaunchProbeHost.tsx', import.meta.url), 'utf8');

/** A `&[&str]` constant's strings, as written in the Rust source. */
function list(name: string): string[] {
  const body = new RegExp(String.raw`pub const ${name}: &\[&str\] = &\[([^\]]*)\];`).exec(probe);
  assert.ok(body, `launch_probe.rs has ${name}`);
  return [...body[1].matchAll(/"([^"]+)"/g)].map(m => m[1]);
}

test('the tour opens every view of the app, in the sidebar order, in both themes', () => {
  assert.deepEqual(list('TOUR_VIEWS'), [...VIEW_TABS]);
  assert.deepEqual(list('TOUR_THEMES'), ['dark', 'light']);
});

test('the tour runs after every check, and only when asked for by name', () => {
  // It comes after the Creator finish, the last of the probe's checks.
  const run = probe.slice(probe.indexOf('async fn run('), probe.indexOf('pub const TOUR_ENV'));
  assert.match(run, /creator_finish\(app, &window, &data_dir, &creation\)\.await\?;\s*if tour_requested\(std::env::var_os\(TOUR_ENV\)\.as_deref\(\)\) \{\s*views_tour\(app, &data_dir\)\.await\?;/);
  assert.match(probe, /pub const TOUR_ENV: &str = "SHODH_LAUNCH_PROBE_TOUR";/);
  assert.match(probe, /env_value == Some\(OsStr::new\(TOUR_VIEWS_NAME\)\)/);
});

test('the main window opens a toured view as the sidebar does, in the theme asked for', () => {
  assert.match(host, /payload\.kind === 'show' && typeof payload\.view === 'string'/);
  // Only one of the app's own views, through the same event a click in the sidebar sends.
  assert.match(host, /const tab = normalizeViewTab\(payload\.view\);\s*if \(!tab\) return;/);
  assert.match(host, /window\.dispatchEvent\(new CustomEvent\('switchTab', \{ detail: tab \}\)\);/);
  // The findings pane of the check before it is closed, so the picture is of the view.
  assert.match(host, /reviewRef\.current\?\.close\(\);/);
  assert.match(host, /payload\.theme !== themeRef\.current\.theme\) \{\s*themeRef\.current\.toggleTheme\(\);/);
});

test('after the views, a scene: Ask with the sample conversation, the sidebar folded', () => {
  assert.deepEqual(list('TOUR_SCENES'), ['conversation', 'run-plan', 'run-progress', 'run-stopped', 'run-summary', 'build-card', 'settings-background', 'code-map']);
  // The sample conversations are written before the first view, as a window saves one.
  const tour = probe.slice(probe.indexOf('async fn views_tour'), probe.indexOf('/// The event main'));
  // The sample project (a workspace with a chat) is there for every view, so each view's
  // chat check runs with a workspace group in the sidebar, as a user's has.
  assert.match(tour, /let run = sample_run\(\)\?;\s*write_sample_conversations\(app, data_dir, &run\)\?;[\s\S]*?write_code_scene\(app, data_dir\)\.await\?;\s*let mut clipped_total/);
  assert.doesNotMatch(probe, /remove_code_scene/);
  assert.match(probe, /\.write_turns\(&sample\.id, writes, &\[\], true\)\?/);
  assert.match(probe, /crate::conversation_commands::emit_changed\(app, &sample\.id\);/);
  assert.match(tour, /"conversation": conversation, "run": run_state/);
  // The window opens it as the sidebar's row does, folds the sidebar, and gives both back
  // to the next view.
  assert.match(host, /SCENES\.has\(payload\.view\) && typeof payload\.conversation === 'string'/);
  assert.match(host, /sidebarRef\.current\.setSidebarCollapsed\(true\);\s*const conversationId = payload\.conversation;\s*sessionRef\.current\.switchConversation\(conversationId\);/);
  assert.match(host, /const SCENES = new Set\(\['conversation', 'run-plan', 'run-progress', 'run-stopped', 'run-summary', 'build-card', 'code-map'\]\);/);
  assert.match(host, /sidebarRef\.current\.setSidebarCollapsed\(before\.collapsed\);\s*if \(before\.conversationId\) sessionRef\.current\.switchConversation\(before\.conversationId\);/);
});

test('the Background part of Settings and the code map details have their scenes', () => {
  // Settings opens at its General section, scrolled to Background, as the section menu opens it.
  assert.match(host, /payload\.kind === 'show' && payload\.view === 'settings-background'/);
  assert.match(host, /requestSettingsSection\('general'\);\s*window\.dispatchEvent\(new CustomEvent\('switchTab', \{ detail: 'settings' \}\)\);/);
  assert.match(host, /\[data-testid="\$\{BACKGROUND_SETTINGS\}"\]`, part => part\.scrollIntoView\(\{ block: 'start' \}\)\)/);
  // The code map scene opens the sample project's Code conversation, then its chip's details.
  assert.match(probe, /\} else if view == "code-map" \{\s*Some\(CODE_CONVERSATION_ID\)/);
  assert.match(probe, /record\.mode = Some\(crate::conversation_commands::ConversationMode::Code\);/);
  assert.match(host, /\[data-testid="\$\{CODE_MAP_CHIP\}"\]\[aria-expanded="false"\]`, chip => chip\.click\(\)\)/);
  // Its details say the nightly refresh ran at the last 02:00.
  assert.match(probe, /\.ran_nightly\(night\);/);
});
