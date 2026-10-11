/**
 * Tool chip text and the add-server form (src/features/tools/format.ts).
 *   node --experimental-strip-types --test app/tests/toolsFormat.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chipLabel, formToConfig, splitCommand } from '../src/features/tools/format.ts';

test('the chip counts working servers, skills and every tool', () => {
  assert.equal(
    chipLabel({ servers: [{ error: null }, { error: null }, { error: 'down' }], skills: [{}], total: 31 }),
    '2 MCP · 1 skill · 31 tools',
  );
  assert.equal(chipLabel({ servers: [], skills: [{}, {}], total: 9 }), '2 skills · 9 tools');
  assert.equal(chipLabel({ servers: [{ error: 'down' }], skills: [], total: 1 }), '1 tool');
});

test('command lines keep quoted parts together', () => {
  assert.deepEqual(splitCommand('npx -y "@scope/server name" --root \'C:/My Code\''), [
    'npx',
    '-y',
    '@scope/server name',
    '--root',
    'C:/My Code',
  ]);
});

test('the form becomes an mcp.json server entry', () => {
  const stdio = formToConfig({
    name: 'github',
    kind: 'stdio',
    target: 'npx -y @modelcontextprotocol/server-github',
    secrets: 'GITHUB_TOKEN=ghp_x\n\n',
  });
  assert.ok('config' in stdio);
  assert.deepEqual(JSON.parse(stdio.config), {
    mcpServers: {
      github: { command: 'npx', args: ['-y', '@modelcontextprotocol/server-github'], env: { GITHUB_TOKEN: 'ghp_x' } },
    },
  });
  const http = formToConfig({ name: 'docs', kind: 'http', target: 'https://mcp.example.com/mcp', secrets: 'Authorization: Bearer t' });
  assert.ok('config' in http);
  assert.deepEqual(JSON.parse(http.config).mcpServers.docs, {
    url: 'https://mcp.example.com/mcp',
    headers: { Authorization: 'Bearer t' },
  });
  assert.ok('error' in formToConfig({ name: 'bad name', kind: 'stdio', target: 'x', secrets: '' }));
  assert.ok('error' in formToConfig({ name: 'a', kind: 'http', target: 'ftp://x', secrets: '' }));
  assert.ok('error' in formToConfig({ name: 'a', kind: 'stdio', target: '  ', secrets: '' }));
  assert.ok('error' in formToConfig({ name: 'a', kind: 'stdio', target: 'x', secrets: 'novalue' }));
});
