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
  checkPath,
  filePart,
  hostTools,
  inGitFolder,
  isWithin,
  pathReadings,
  pathProblem,
  toolCallProblem,
} from '../../crates/shodh-rag/src/harness/code_guard.js';

/** A code folder with `src/lib.rs`, next to a secret outside it. */
function fixture(name = 'project') {
  const base = realpathSync.native(mkdtempSync(path.join(tmpdir(), 'shodh-guard-')));
  const root = path.join(base, name);
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
    // Backslash separators and UNC paths only mean something on Windows;
    // elsewhere a backslash is refused (see the omp forms test).
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

test('selectors and copied tags are not part of the file path; other colons are not checked', () => {
  assert.equal(filePart('src/a.rs:10-20'), 'src/a.rs');
  assert.equal(filePart('src/a.rs:raw:1-50'), 'src/a.rs');
  assert.equal(filePart('src/a.rs:-40'), 'src/a.rs');
  assert.equal(filePart('[src/a.rs#00FF]'), 'src/a.rs');
  // omp splits at the last colon, and only before a selector: anything else
  // (archive members, NTFS streams, a colon inside the path) has no file part.
  assert.equal(filePart('archive.zip:inner/file.txt'), null);
  assert.equal(filePart('src/a.rs:../../x'), null);
  assert.equal(filePart('a:b/../../x'), null);
  assert.equal(filePart(':10'), null);
  if (process.platform === 'win32') {
    assert.equal(filePart('C:\\code\\a.rs:5'), 'C:\\code\\a.rs');
    assert.equal(filePart('C:\\code\\a.rs::$DATA'), null);
  } else {
    assert.equal(filePart('C:/code/a.rs:5'), null);
  }
  assert.equal(isWithin('/code/app', '/code/app'), true);
  assert.equal(isWithin('/code/app', '/code/app-old/x'), false);
});

test('forms omp reads differently from their text are refused (omp 18.4.10 expandPath)', () => {
  const { base, root, cleanup } = fixture();
  try {
    const refused = [
      // omp drops a leading ':' or '@' before it resolves the rest.
      ':../secret.txt',
      ':' + path.join(base, 'secret.txt'),
      ':C:\\x',
      '@/etc/passwd',
      '@C:\\x',
      '@' + path.join(base, 'secret.txt'),
      '@src/lib.rs',
      // omp strips quotes from list entries; "" leaves no file at all.
      '""',
      '"../secret.txt"',
      // A colon that is not a selector: omp opens the whole text.
      'src/lib.rs:../../secret.txt',
      'src:x/../../../secret.txt',
      'archive.zip:inner/file.txt',
      // omp reads Unicode spaces as spaces.
      '~\u00A0/x',
    ];
    if (process.platform === 'win32') {
      refused.push('C:secret.txt', 'src\\..\\.. \\secret.txt', '..\u2002\\secret.txt', '.git.\\config', 'src\\lib.rs::$DATA');
    } else {
      // omp's search tools turn backslashes into slashes and its reads undo
      // shell escapes: neither is a plain file name here.
      refused.push('\\.\\./secret.txt', '..\\secret.txt', 'src\\ dir/x');
    }
    for (const p of refused) {
      assert.ok(pathProblem(root, root, p), p);
    }
    // A path of only slashes is the code folder for omp.
    for (const p of ['src/lib.rs', 'src/lib.rs:10-20', '/']) {
      assert.equal(pathProblem(root, root, p), null, p);
    }
  } finally {
    cleanup();
  }
});

test('on Windows, /c/... names a drive as omp reads it', { skip: process.platform !== 'win32' }, () => {
  const { base, root, cleanup } = fixture();
  try {
    const alias = (p: string) => '/' + p[0].toLowerCase() + p.slice(2).replace(/\\/g, '/');
    assert.equal(pathProblem(root, root, alias(path.join(root, 'src', 'lib.rs'))), null);
    assert.match(pathProblem(root, root, alias(path.join(base, 'secret.txt'))) ?? '', /outside/);
    assert.match(pathProblem(root, root, '/mnt' + alias(path.join(base, 'secret.txt'))) ?? '', /outside/);
  } finally {
    cleanup();
  }
});

test('every way omp may split a path list is checked', () => {
  const { root, cleanup } = fixture();
  try {
    // A list omp would split: every part of every split it may take.
    assert.deepEqual(pathReadings(root, 'src, tests'), ['src', 'tests']);
    assert.deepEqual(pathReadings(root, 'src/lib.rs'), ['src/lib.rs']);
    for (const value of ['src, ../', 'src ../', 'src\t../secret.txt', 'src,"../secret.txt"', 'src;../', 'src @/etc']) {
      assert.ok(toolCallProblem(root, root, 'grep', { pattern: 'x', path: value }), value);
      assert.ok(toolCallProblem(root, root, 'glob', { path: value }), value);
    }
    assert.equal(toolCallProblem(root, root, 'grep', { pattern: 'fn', path: 'src, src/lib.rs' }), null);
    // read, write and edit take one path; a read of a missing ';' list is refused.
    assert.ok(toolCallProblem(root, root, 'read', { path: 'src/lib.rs;../secret.txt' }));
    // omp's native glob may expand braces.
    assert.match(toolCallProblem(root, root, 'glob', { path: 'src/{..,x}/../secret.txt' }) ?? '', /braces/);
    assert.match(toolCallProblem(root, root, 'grep', { pattern: 'x', path: '{../..,src}' }) ?? '', /braces/);
    assert.equal(toolCallProblem(root, root, 'glob', { path: 'src/**/*.{rs,ts}' }), null);
  } finally {
    cleanup();
  }
});

test('a code folder whose path has spaces works with absolute paths', () => {
  const { base, root, cleanup } = fixture('My Project');
  try {
    const lib = path.join(root, 'src', 'lib.rs');
    assert.equal(toolCallProblem(root, root, 'read', { path: lib }), null);
    assert.equal(toolCallProblem(root, root, 'write', { path: path.join(root, 'src', 'new file.rs'), content: 'x' }), null);
    assert.equal(toolCallProblem(root, root, 'edit', { path: lib, old_string: 'a', new_string: 'b' }), null);
    assert.equal(toolCallProblem(root, root, 'grep', { pattern: 'fn', path: path.join(root, 'src') }), null);
    assert.equal(toolCallProblem(root, root, 'glob', { path: path.join(root, 'src') + '/**/*.rs' }), null);
    assert.equal(toolCallProblem(root, root, 'bash', { command: 'ls', cwd: path.join(root, 'src') }), null);
    assert.match(toolCallProblem(root, root, 'read', { path: path.join(base, 'secret.txt') }) ?? '', /outside/);
  } finally {
    cleanup();
  }
});

test('a link that points nowhere is not written through', (t) => {
  const { base, root, cleanup } = fixture();
  try {
    const link = path.join(root, 'src', 'notes');
    const target = path.join(base, 'not-yet', 'x');
    try {
      symlinkSync(target, link, 'file');
    } catch {
      try {
        // Windows without the symlink privilege: a junction to a missing folder.
        symlinkSync(target, link, 'junction');
      } catch {
        t.skip('links cannot be created here');
        return;
      }
    }
    for (const p of ['src/notes', 'src/notes/deeper.txt']) {
      assert.match(toolCallProblem(root, root, 'write', { path: p, content: 'x' }) ?? '', /outside|points nowhere/, p);
    }
  } finally {
    cleanup();
  }
});

test('reads and edits need an existing file; writes may create one', () => {
  const { root, cleanup } = fixture();
  try {
    // omp looks for a missing file elsewhere in the folder (by its suffix),
    // which the guard cannot follow.
    assert.match(toolCallProblem(root, root, 'read', { path: 'lib.rs' }) ?? '', /does not exist/);
    assert.match(toolCallProblem(root, root, 'edit', { path: 'nope.rs', old_string: 'a', new_string: 'b' }) ?? '', /does not exist/);
    assert.equal(toolCallProblem(root, root, 'read', { path: 'src/lib.rs:1-5' }), null);
    assert.equal(toolCallProblem(root, root, 'write', { path: 'src/new/file.rs', content: 'x' }), null);
    assert.equal(checkPath(root, root, 'src/lib.rs').resolved, path.join(root, 'src', 'lib.rs'));
  } finally {
    cleanup();
  }
});

test('edits and writes never touch the .git folder; other files report their target', () => {
  const { root, cleanup } = fixture();
  try {
    mkdirSync(path.join(root, '.git', 'hooks'), { recursive: true });
    writeFileSync(path.join(root, '.git', 'config'), '[core]');
    for (const p of ['.git/config', '.GIT/hooks/post-checkout', 'src/../.git/config', '.git', 'sub/.Git/x']) {
      assert.match(toolCallProblem(root, root, 'write', { path: p, content: 'x' }) ?? '', /\.git folder/, p);
    }
    assert.match(toolCallProblem(root, root, 'edit', { path: '.git/config', old_string: 'a', new_string: 'b' }) ?? '', /\.git folder/);
    // Reading git's files is allowed.
    assert.equal(toolCallProblem(root, root, 'read', { path: '.git/config' }), null);
    assert.equal(inGitFolder(root, path.join(root, '.gitignore')), false);
    // Files that can run programs pass here with their target: Shodh's
    // approval decides (code_mode.rs `execution_risk`).
    for (const p of ['.vscode/tasks.json', '.envrc', 'package.json']) {
      let target: string | undefined;
      const problem = toolCallProblem(root, root, 'write', { path: p, content: 'x' }, (t: string) => {
        target = t;
      });
      assert.equal(problem, null, p);
      assert.equal(target, path.join(root, p));
    }
    assert.match(toolCallProblem(root, root, 'edit', { path: 'src/lib.rs', _input: '[../secret.txt#1A2B]' }) ?? '', /single-file replace/);
    assert.match(toolCallProblem(root, root, 'edit', { path: 'src/lib.rs', _path: '../secret.txt', old_string: 'a', new_string: 'b' }) ?? '', /one file/);
  } finally {
    cleanup();
  }
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

    // An edit or a write tells Shodh the file it lands in.
    assert.equal(await allowed.call(root, 'write', { path: 'src/new.rs', content: 'x' }), undefined);
    assert.equal(JSON.parse(allowed.asked[1].text).target, path.join(root, 'src', 'new.rs'));
    allowed.asked.length = 1;

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
