/**
 * Code mode's tokens-saved words (src/features/codeIntel/format.ts).
 *   node --experimental-strip-types --test app/tests/codeIntel.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  approxTokens,
  chipLabel,
  savedShare,
  sessionDetail,
  sessionSentence,
  showsChip,
  totalDetail,
} from '../src/features/codeIntel/format.ts';

const counted = {
  savedTokens: 41_200,
  originalTokens: 50_000,
  sentTokens: 8_800,
  commands: 12,
  shortenedCommands: 9,
  compressionOn: true,
};

test('every number is an estimate and says so', () => {
  assert.equal(approxTokens(41_200), '≈41,200');
  assert.equal(approxTokens(0), '≈0');
  assert.equal(approxTokens(-3), '≈0');
  assert.equal(chipLabel(counted), '≈41,200 tokens saved');
  assert.equal(sessionSentence(counted), 'Saved ≈41,200 tokens in this conversation');
  assert.equal(
    sessionDetail(counted),
    'The model read ≈8,800 tokens of command output instead of ≈50,000, over 12 commands.',
  );
});

test('the chip shows from the start of a Code conversation, so its switch is reachable', () => {
  assert.equal(showsChip(null), false);
  const fresh = { ...counted, savedTokens: 0, originalTokens: 0, sentTokens: 0, commands: 0, shortenedCommands: 0 };
  assert.equal(showsChip(fresh), true);
  assert.equal(chipLabel(fresh), 'Output shortening on');
  assert.equal(showsChip(counted), true);
  const off = { ...fresh, compressionOn: false };
  assert.equal(showsChip(off), true);
  assert.equal(chipLabel(off), 'Output shortening off');
  assert.equal(sessionDetail(fresh), 'No command has run in this conversation yet.');
});

test('the running total reads as a share and a count', () => {
  assert.equal(savedShare(counted), '82%');
  assert.equal(savedShare({ ...counted, originalTokens: 0 }), null);
  assert.equal(totalDetail({ ...counted, conversations: 3 }), '82% of command output · 12 commands in 3 conversations');
  assert.equal(
    totalDetail({ ...counted, commands: 1, conversations: 1 }),
    '82% of command output · 1 command in 1 conversation',
  );
  assert.equal(
    totalDetail({ ...counted, commands: 0, conversations: 0, originalTokens: 0, savedTokens: 0 }),
    'Counted from the first Code mode command',
  );
});
