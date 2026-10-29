#!/usr/bin/env node
// The app version lives in eight places across six files. This reads them
// all, checks they agree (and match a tag), and sets them all at once.
//
//   node scripts/release/versions.mjs check [--tag vX.Y.Z] [--root DIR] [--github-output]
//   node scripts/release/versions.mjs set X.Y.Z [--root DIR]
//
// Each value is found by parsing the file's structure (a JSON path; the
// [package] table of a manifest; the [[package]] entry of Cargo.lock with
// that name and no registry source), never by line number. `set` rewrites
// only the characters of each version string, so the files keep their
// formatting, then re-reads every file and checks that nothing else changed.

import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { isDeepStrictEqual, parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

export const LOCATIONS = [
  { file: 'app/package.json', kind: 'json', path: ['version'] },
  { file: 'app/package-lock.json', kind: 'json', path: ['version'] },
  { file: 'app/package-lock.json', kind: 'json', path: ['packages', '', 'version'] },
  { file: 'app/src-tauri/tauri.conf.json', kind: 'json', path: ['version'] },
  { file: 'app/src-tauri/Cargo.toml', kind: 'manifest', package: 'shodh-rag-app' },
  { file: 'crates/shodh-rag/Cargo.toml', kind: 'manifest', package: 'shodh-rag' },
  { file: 'Cargo.lock', kind: 'lock', package: 'shodh-rag-app' },
  { file: 'Cargo.lock', kind: 'lock', package: 'shodh-rag' },
];

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

export class VersionError extends Error {}

export function describe(location) {
  if (location.kind === 'json') return `${location.file} ${location.path.map((p) => JSON.stringify(p)).join(' > ')}`;
  if (location.kind === 'manifest') return `${location.file} [package] version`;
  return `${location.file} [[package]] ${location.package}`;
}

export function parseVersion(text) {
  const m = SEMVER.exec(text);
  if (!m) throw new VersionError(`${JSON.stringify(text)} is not a version of the form X.Y.Z`);
  return m.slice(1).map(Number);
}

export function compareVersions(a, b) {
  const [x, y] = [parseVersion(a), parseVersion(b)];
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
  return 0;
}

// --- JSON -------------------------------------------------------------------

/**
 * The span [start, end) of the characters inside the quotes of the string at
 * `path`, found by walking the document's structure.
 */
export function locateJsonString(text, path, what = 'the JSON document') {
  let i = 0;
  const found = [];
  const fail = (message) => {
    throw new VersionError(`${what}: ${message} at offset ${i}`);
  };
  const ws = () => {
    while (i < text.length && ' \t\n\r'.includes(text[i])) i++;
  };
  const expect = (c) => {
    ws();
    if (text[i] !== c) fail(`expected ${JSON.stringify(c)}`);
    i++;
  };
  const string = () => {
    if (text[i] !== '"') fail('expected a string');
    const start = i;
    i++;
    while (text[i] !== '"') {
      if (i >= text.length || text[i] === '\n') fail('unterminated string');
      if (text[i] === '\\') i++;
      i++;
    }
    i++;
    return [start, i];
  };
  const same = (a) => a.length === path.length && a.every((p, k) => p === path[k]);
  const literal = /true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/y;
  const value = (at) => {
    ws();
    const c = text[i];
    if (c === '{') {
      i++;
      ws();
      if (text[i] === '}') return void i++;
      for (;;) {
        ws();
        const [ks, ke] = string();
        const key = JSON.parse(text.slice(ks, ke));
        expect(':');
        value([...at, key]);
        ws();
        if (text[i] === ',') {
          i++;
          continue;
        }
        expect('}');
        return;
      }
    }
    if (c === '[') {
      i++;
      ws();
      if (text[i] === ']') return void i++;
      for (let n = 0; ; n++) {
        value([...at, n]);
        ws();
        if (text[i] === ',') {
          i++;
          continue;
        }
        expect(']');
        return;
      }
    }
    if (c === '"') {
      const [s, e] = string();
      if (same(at)) found.push([s + 1, e - 1]);
      return;
    }
    literal.lastIndex = i;
    const m = literal.exec(text);
    if (!m) fail('unexpected character');
    if (same(at)) fail(`${path.join('.')} is not a string`);
    i += m[0].length;
  };
  value([]);
  ws();
  if (i !== text.length) fail('trailing characters');
  if (found.length !== 1) {
    throw new VersionError(`${what}: ${found.length === 0 ? 'no' : 'more than one'} string at ${JSON.stringify(path)}`);
  }
  const [start, end] = found[0];
  return { start, end, value: JSON.parse(`"${text.slice(start, end)}"`) };
}

// --- TOML (the two shapes Cargo writes) ------------------------------------

function* linesWithOffsets(text) {
  let offset = 0;
  for (const line of text.split('\n')) {
    yield { line, offset };
    offset += line.length + 1;
  }
}

const HEADER = /^\s*(\[\[?)\s*([A-Za-z0-9_.-]+)\s*(\]\]?)\s*(?:#.*)?$/;
const STRING_KEY = /^(\s*)([A-Za-z0-9_-]+)(\s*=\s*)"([^"\\\n]*)"\s*(?:#.*)?$/;

function tomlTables(text) {
  const tables = [];
  let current = { header: null, array: false, keys: new Map(), dotted: new Set() };
  tables.push(current);
  for (const { line: raw, offset } of linesWithOffsets(text)) {
    // A CRLF file: matching ignores the \r; offsets still count it.
    const line = raw.endsWith('\r') ? raw.slice(0, -1) : raw;
    if (/^\s*\[/.test(line)) {
      // Every header starts a new table; one this does not read (a quoted
      // key, as in [target.'cfg(windows)'.dependencies]) is never [package].
      const header = HEADER.exec(line);
      const array = header ? header[1] === '[[' : false;
      if (header && array !== (header[3] === ']]')) throw new VersionError(`Malformed table header: ${line}`);
      current = { header: header ? header[2] : null, array, keys: new Map(), dotted: new Set() };
      tables.push(current);
      continue;
    }
    const kv = STRING_KEY.exec(line);
    if (kv) {
      const start = offset + kv[1].length + kv[2].length + kv[3].length + 1;
      if (current.keys.has(kv[2])) throw new VersionError(`Duplicate key ${kv[2]} in [${current.header}]`);
      current.keys.set(kv[2], { start, end: start + kv[4].length, value: kv[4] });
      continue;
    }
    const dotted = /^\s*([A-Za-z0-9_-]+)\s*\./.exec(line);
    if (dotted) current.dotted.add(dotted[1]);
  }
  return tables;
}

export function locateManifestVersion(text, expectedName, what = 'Cargo.toml') {
  const packages = tomlTables(text).filter((t) => t.header === 'package' && !t.array);
  if (packages.length !== 1) throw new VersionError(`${what}: expected one [package] table, found ${packages.length}`);
  const [table] = packages;
  if (table.dotted.has('version')) throw new VersionError(`${what}: the version is inherited (version.workspace), not set here`);
  const name = table.keys.get('name');
  if (!name || name.value !== expectedName) {
    throw new VersionError(`${what}: [package] name is ${name ? JSON.stringify(name.value) : 'missing'}, expected ${JSON.stringify(expectedName)}`);
  }
  const version = table.keys.get('version');
  if (!version) throw new VersionError(`${what}: [package] has no version = "..."`);
  return version;
}

export function locateLockVersion(text, packageName, what = 'Cargo.lock') {
  const entries = tomlTables(text).filter(
    (t) => t.header === 'package' && t.array && t.keys.get('name')?.value === packageName && !t.keys.has('source'),
  );
  if (entries.length !== 1) {
    throw new VersionError(`${what}: expected one workspace [[package]] named ${packageName}, found ${entries.length}`);
  }
  const version = entries[0].keys.get('version');
  if (!version) throw new VersionError(`${what}: [[package]] ${packageName} has no version`);
  return version;
}

// --- Reading and writing ----------------------------------------------------

export function locate(text, location) {
  const what = describe(location);
  if (location.kind === 'json') return locateJsonString(text, location.path, what);
  if (location.kind === 'manifest') return locateManifestVersion(text, location.package, what);
  return locateLockVersion(text, location.package, what);
}

function readTexts(root) {
  const texts = new Map();
  for (const { file } of LOCATIONS) {
    if (!texts.has(file)) texts.set(file, readFileSync(join(root, file), 'utf8'));
  }
  return texts;
}

export function readVersions(texts) {
  return LOCATIONS.map((location) => ({ location, ...locate(texts.get(location.file), location) }));
}

/** The one version all locations agree on; throws listing them when they differ. */
export function agreedVersion(found) {
  const distinct = [...new Set(found.map((f) => f.value))];
  if (distinct.length !== 1) {
    const table = found.map((f) => `  ${f.value.padEnd(12)} ${describe(f.location)}`).join('\n');
    throw new VersionError(`The version differs between files:\n${table}`);
  }
  parseVersion(distinct[0]);
  return distinct[0];
}

function setJsonPath(doc, path, value) {
  let node = doc;
  for (const key of path.slice(0, -1)) node = node[key];
  node[path[path.length - 1]] = value;
}

/** Returns the new texts with every version set to `version`, checked. */
export function applyVersion(texts, version) {
  parseVersion(version);
  const found = readVersions(texts);
  const current = agreedVersion(found);
  if (compareVersions(version, current) <= 0) {
    throw new VersionError(`The new version ${version} must be greater than the current ${current}`);
  }
  const updated = new Map(texts);
  for (const file of new Set(LOCATIONS.map((l) => l.file))) {
    const spans = found.filter((f) => f.location.file === file).sort((a, b) => b.start - a.start);
    let text = texts.get(file);
    for (const span of spans) text = text.slice(0, span.start) + version + text.slice(span.end);
    updated.set(file, text);
  }
  checkOnlyVersionsChanged(texts, updated, version);
  return { previous: current, texts: updated };
}

function checkOnlyVersionsChanged(before, after, version) {
  for (const location of LOCATIONS) {
    const now = locate(after.get(location.file), location);
    if (now.value !== version) throw new VersionError(`${describe(location)} reads ${now.value} after the update`);
  }
  for (const file of new Set(LOCATIONS.map((l) => l.file))) {
    const [old, neu] = [before.get(file), after.get(file)];
    if (file.endsWith('.json')) {
      const expected = JSON.parse(old);
      for (const l of LOCATIONS.filter((x) => x.file === file)) setJsonPath(expected, l.path, version);
      if (!isDeepStrictEqual(JSON.parse(neu), expected)) throw new VersionError(`${file}: more than the version changed`);
    } else {
      const [a, b] = [old.split('\n'), neu.split('\n')];
      if (a.length !== b.length) throw new VersionError(`${file}: the number of lines changed`);
      const changed = a.filter((line, k) => line !== b[k]).length;
      const expected = LOCATIONS.filter((x) => x.file === file).length;
      if (changed !== expected) throw new VersionError(`${file}: ${changed} lines changed, expected ${expected}`);
    }
  }
}

// --- CLI --------------------------------------------------------------------

function main(argv) {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      root: { type: 'string', default: '.' },
      tag: { type: 'string' },
      'github-output': { type: 'boolean', default: false },
    },
  });
  const [command, arg] = positionals;
  const texts = readTexts(values.root);
  if (command === 'check') {
    const found = readVersions(texts);
    for (const f of found) console.log(`${f.value.padEnd(12)} ${describe(f.location)}`);
    const version = agreedVersion(found);
    if (values.tag !== undefined && values.tag !== `v${version}`) {
      throw new VersionError(`The tag ${values.tag} does not match the version ${version} in every file (expected v${version})`);
    }
    console.log(`Version ${version}${values.tag ? `, matches ${values.tag}` : ''}`);
    if (values['github-output']) appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\ntag=v${version}\n`);
    return;
  }
  if (command === 'set') {
    if (!arg) throw new VersionError('Usage: versions.mjs set X.Y.Z');
    const { previous, texts: updated } = applyVersion(texts, arg);
    for (const [file, text] of updated) if (text !== texts.get(file)) writeFileSync(join(values.root, file), text);
    console.log(`Version ${previous} -> ${arg} in ${LOCATIONS.length} places:`);
    for (const l of LOCATIONS) console.log(`  ${describe(l)}`);
    return;
  }
  throw new VersionError('Usage: versions.mjs check [--tag vX.Y.Z] | set X.Y.Z');
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    if (!(e instanceof VersionError)) throw e;
    console.error(`::error::${e.message.replace(/\n/g, '%0A')}`);
    process.exit(1);
  }
}
