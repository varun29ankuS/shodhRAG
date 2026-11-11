// The NOTICE the app ships with: office_oxide's entry says which of its two
// licences Shodh takes and carries that licence's text. Read here, not by a
// crate's test: NOTICE is documentation to the heavy jobs' change filter, and a
// crate that included it would make it a build input.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const notice = readFileSync(join(ROOT, 'NOTICE'), 'utf8').replace(/\r\n/g, '\n');

test('office_oxide ships with the licence it is used under', () => {
  const parts = notice.split('\nOfficeOxide\n');
  assert.ok(parts.length > 1, 'NOTICE has an OfficeOxide entry');
  const entry = parts[1].split('\n\n')[0];
  assert.match(entry, /version 0\.1\.10/);
  assert.match(entry, /used under MIT/);
  assert.match(entry, /Permission is hereby granted, free of charge/);
  assert.match(entry, /THE SOFTWARE IS PROVIDED "AS IS"/);
});
