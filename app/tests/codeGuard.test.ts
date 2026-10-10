/**
 * Code mode guard tests: the omp hook that keeps file tools inside the code
 * folder (crates/shodh-rag/src/harness/code_guard.js). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/codeGuard.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import guard, {
  ALLOW,
  APPROVAL_TITLE,
  GUARD_COMMAND,
  HOST_TOOLS_ENV,
  ROOT_ENV,
  filePart,
  hostTools,
  isWithin,
  pathProblem,
  toolCallProblem,
} from '../../crates/shodh-rag/src/harness/code_guard.js';

/** A code folder with `src/lib.rs`, next to a secret outside it. */
function fixture() {
  const base = realpathSync.native(mkdtempSync(path.join(tmpdir(), 'shodh-guard-')));
  const root = path.join(base, 'project');
  mkdirSync(path.join(root, 'src'), { recursive: true });
  writeFileSync(path.join(root, 'src', 'lib.rs'), 'fn a() {}');
  writeFileSync(path.join(base, 'secret.txt'), 'secret');
  return { base, root, cleanup: () => rmSync(base, { recursive: true, force: true }) };
}

test('paths inside the code folder are allowed, including files that do not exist yet', () => {
  const { root, cleanup } = fixture();
  try {
    for (const p of ['src/lib.rs', './src/lib.rs', 'src/new/deep/file.rs', path.join(root, 'src', 'lib.rs'), '.', '', 'src/**/*.rs', 'src/lib.rs:10-20', '[src/lib.rs#1A2B]']) {
      assert.equal(pathProblem(root, root, p), null, p);
    }
  } finally {
    cleanup();
  }
});

test('paths that leave the code folder are blocked', () => {
  const { base, root, cleanup } = fixture();
  try {
    // Backslash separators and UNC paths only mean something on Windows; on
    // other systems a backslash is part of a file name inside the folder.
    const windowsOnly = process.platform === 'win32' ? ['..\\secret.txt', '\\\\server\\share\\x'] : [];
    for (const p of [
      '../secret.txt',
      ...windowsOnly,
      path.join(base, 'secret.txt'),
      'src/../../secret.txt',
      '**/../../secret.txt',
      '~/.ssh/id_rsa',
      '//server/share/x',
      root + '-sibling/x',
    ]) {
      assert.match(pathProblem(root, root, p) ?? '', /outside the code folder|network path/, p);
    }
  } finally {
    cleanup();
  }
});

test('URLs and omp resources are blocked', () => {
  const { root, cleanup } = fixture();
  try {
    for (const p of ['https://example.com', 'ssh://host/etc/passwd', 'local://notes.md', 'proc://1/kill', 'omp://rpc.md', 'xd://lsp']) {
      assert.match(pathProblem(root, root, p) ?? '', /URL or internal resource/, p);
    }
  } finally {
    cleanup();
  }
});

test('links inside the folder that point outside are blocked', (t) => {
  const { base, root, cleanup } = fixture();
  try {
    try {
      symlinkSync(base, path.join(root, 'escape'), process.platform === 'win32' ? 'junction' : 'dir');
    } catch {
      t.skip('links cannot be created here');
      return;
    }
    assert.match(pathProblem(root, root, 'escape/secret.txt') ?? '', /outside the code folder/);
  } finally {
    cleanup();
  }
});

test('only the Code tools run, each with its paths checked', () => {
  const { root, cleanup } = fixture();
  try {
    assert.equal(toolCallProblem(root, root, 'read', { path: 'src/lib.rs' }), null);
    assert.equal(toolCallProblem(root, root, 'write', { path: 'src/new.rs', content: 'x' }), null);
    assert.equal(toolCallProblem(root, root, 'edit', { path: 'src/lib.rs', old_string: 'a', new_string: 'b' }), null);
    assert.equal(toolCallProblem(root, root, 'grep', { pattern: 'fn', path: 'src; tests' }), null);
    assert.equal(toolCallProblem(root, root, 'grep', { pattern: 'fn' }), null);
    assert.equal(toolCallProblem(root, root, 'glob', { path: 'src/**/*.rs' }), null);
    assert.equal(toolCallProblem(root, root, 'ast_grep', { pat: 'fn $A() {}' }), null);
    assert.equal(toolCallProblem(root, root, 'bash', { command: 'cargo test' }), null);
    assert.equal(toolCallProblem(root, root, 'bash', { command: 'ls', cwd: 'src' }), null);

    assert.match(toolCallProblem(root, root, 'grep', { pattern: 'x', path: 'src; ../' }) ?? '', /outside/);
    assert.match(toolCallProblem(root, root, 'edit', { path: 'src/lib.rs', paths: ['../secret.txt'] }) ?? '', /outside/);
    assert.match(toolCallProblem(root, root, 'edit', { input: '[../secret.txt#1A2B]\nREM' }) ?? '', /single-file replace/);
    assert.match(toolCallProblem(root, root, 'write', { content: 'x' }) ?? '', /needs a file path/);
    assert.match(toolCallProblem(root, root, 'bash', { command: 'ls', cwd: '..' }) ?? '', /outside/);
    assert.match(toolCallProblem(root, root, 'bash', { command: ' ' }) ?? '', /needs a command/);
    for (const tool of ['task', 'eval', 'browser', 'python', 'web_search', 'ast_edit', 'lsp']) {
      assert.match(toolCallProblem(root, root, tool, {}) ?? '', /not available in Code mode/, tool);
    }
  } finally {
    cleanup();
  }
});

test('selectors and copied tags are not part of the file path', () => {
  assert.equal(filePart('src/a.rs:10-20'), 'src/a.rs');
  assert.equal(filePart('C:\\code\\a.rs:5'), 'C:\\code\\a.rs');
  assert.equal(filePart('[src/a.rs#00FF]'), 'src/a.rs');
  assert.equal(filePart('archive.zip:inner/file.txt'), 'archive.zip');
  assert.equal(isWithin('/code/app', '/code/app'), true);
  assert.equal(isWithin('/code/app', '/code/app-old/x'), false);
});

/** The hook registered with a fake omp. */
function loadGuard(answer: (title: string, text: string) => Promise<string | undefined>) {
  let handler: ((event: unknown, ctx: unknown) => Promise<unknown>) | null = null;
  const commands: string[] = [];
  guard({
    registerCommand: (name: string) => commands.push(name),
    on: (event: string, fn: (event: unknown, ctx: unknown) => Promise<unknown>) => {
      if (event === 'tool_call') handler = fn;
    },
  });
  assert.deepEqual(commands, [GUARD_COMMAND]);
  assert.ok(handler);
  const asked: Array<{ title: string; text: string }> = [];
  const call = (root: string, toolName: string, input: Record<string, unknown>, hasUI = true) =>
    handler!(
      { toolName, toolCallId: 'call-1', input },
      {
        cwd: root,
        hasUI,
        ui: {
          input: (title: string, text: string) => {
            asked.push({ title, text });
            return answer(title, text);
          },
        },
      },
    );
  return { call, asked };
}

test('changes wait for the approval answer; reads do not ask', async () => {
  const { root, cleanup } = fixture();
  const previous = process.env[ROOT_ENV];
  process.env[ROOT_ENV] = root;
  try {
    const allowed = loadGuard(async () => ALLOW);
    assert.equal(await allowed.call(root, 'read', { path: 'src/lib.rs' }), undefined);
    assert.equal(allowed.asked.length, 0);
    assert.equal(await allowed.call(root, 'bash', { command: 'cargo test' }), undefined);
    assert.equal(allowed.asked.length, 1);
    assert.equal(allowed.asked[0].title, APPROVAL_TITLE);
    assert.deepEqual(JSON.parse(allowed.asked[0].text), { toolCallId: 'call-1', tool: 'bash', input: { command: 'cargo test' } });

    const declined = loadGuard(async () => 'The user declined this.');
    assert.deepEqual(await declined.call(root, 'write', { path: 'a.txt', content: 'x' }), { block: true, reason: 'The user declined this.' });
    const closed = loadGuard(async () => undefined);
    assert.deepEqual(await closed.call(root, 'edit', { path: 'src/lib.rs', old_string: 'a', new_string: 'b' }), { block: true, reason: 'The user did not approve this.' });
    // Blocked paths are never asked about; without a dialog channel nothing changes.
    assert.equal((await allowed.call(root, 'write', { path: '../x', content: 'x' }) as { block: boolean }).block, true);
    assert.equal(allowed.asked.length, 1);
    assert.equal((await allowed.call(root, 'bash', { command: 'ls' }, false) as { block: boolean }).block, true);

    delete process.env[ROOT_ENV];
    assert.equal((await allowed.call(root, 'read', { path: 'src/lib.rs' }) as { block: boolean }).block, true);
  } finally {
    if (previous === undefined) delete process.env[ROOT_ENV];
    else process.env[ROOT_ENV] = previous;
    cleanup();
  }
});

test('host tools of the session pass to Shodh; nothing else does', async () => {
  assert.deepEqual([...hostTools('enola__explore, load_skill,,')], ['enola__explore', 'load_skill']);
  // A host tool can never take the name of a Code tool and skip its checks.
  assert.deepEqual([...hostTools('bash,read,x__y')], ['x__y']);
  assert.deepEqual([...hostTools(undefined)], []);

  const { root, cleanup } = fixture();
  const previous = { root: process.env[ROOT_ENV], host: process.env[HOST_TOOLS_ENV] };
  process.env[ROOT_ENV] = root;
  process.env[HOST_TOOLS_ENV] = 'enola__explore,bash';
  try {
    const guarded = loadGuard(async () => ALLOW);
    // Shodh asks and audits host tools itself: no dialog here, no path check.
    assert.equal(await guarded.call(root, 'enola__explore', { repo_path: '..' }), undefined);
    assert.equal(guarded.asked.length, 0);
    // A tool not named for this session is still blocked.
    assert.equal((await guarded.call(root, 'other__tool', {}) as { block: boolean }).block, true);
    // bash stays guarded although the variable names it.
    assert.equal(await guarded.call(root, 'bash', { command: 'ls' }), undefined);
    assert.equal(guarded.asked.length, 1);
  } finally {
    for (const [name, value] of [[ROOT_ENV, previous.root], [HOST_TOOLS_ENV, previous.host]] as const) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
    cleanup();
  }
});
