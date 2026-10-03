/**
 * SVG sketch sanitizer policy.
 *   node --experimental-strip-types --test app/tests/visualSvg.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SVG_MAX_CHARS, compactSvg, sanitizeSvg, wantsSketch } from '../src/features/ask/visual/svgSanitize.ts';

function clean(source: string, idPrefix = 't') {
  const r = sanitizeSvg(source, { idPrefix });
  assert.ok(r.ok, r.ok ? '' : r.error);
  return r.ok ? r : (null as never);
}

const FBD = `<svg viewBox="0 0 200 120" xmlns="http://www.w3.org/2000/svg">
  <title>Block on an incline</title>
  <defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z"/></marker></defs>
  <polygon points="10,110 190,110 190,30" fill="none" stroke="black"/>
  <rect x="120" y="50" width="30" height="20" transform="rotate(-24 135 60)" fill="white" stroke="black"/>
  <line x1="135" y1="60" x2="135" y2="100" stroke="#000" marker-end="url(#arrow)"/>
  <circle cx="20" cy="20" r="5"/><ellipse cx="50" cy="20" rx="8" ry="4"/>
  <path d="M10 10 Q 20 0 30 10" fill="none" stroke="currentColor" stroke-width="2"/>
  <text x="140" y="112" font-size="10">mg</text>
</svg>`;

test('svg: keeps valid shapes, text, markers and the title', () => {
  const r = clean(FBD);
  for (const tag of ['<polygon', '<rect', '<line', '<circle', '<ellipse', '<path', '<text', '<marker']) assert.ok(r.svg.includes(tag), tag);
  assert.ok(r.svg.includes('>mg</text>'));
  assert.equal(r.title, 'Block on an incline');
  assert.equal(r.width, 200);
  assert.equal(r.height, 120);
  assert.deepEqual(r.removed, []);
});

test('svg: ids are prefixed and local references follow', () => {
  const r = clean(FBD, 'abc');
  assert.ok(r.svg.includes('id="abc-arrow"'));
  assert.ok(r.svg.includes('marker-end="url(#abc-arrow)"'));
  const use = clean('<svg viewBox="0 0 10 10"><defs><circle id="c" r="1"/></defs><use href="#c" x="2"/><use xlink:href="#c" x="4"/></svg>', 'p');
  assert.equal((use.svg.match(/href="#p-c"/g) ?? []).length, 2);
});

test('svg: black ink follows the theme text colour, white the surface', () => {
  const r = clean(FBD);
  assert.ok(r.svg.includes('stroke="currentColor"'));
  assert.ok(!/stroke="(black|#000)"/.test(r.svg));
  assert.ok(r.svg.includes('fill: var(--c-surface)'));
  // Unset fill on the root becomes the text colour (SVG's default is black).
  assert.match(r.svg, /^<svg [^>]*fill="currentColor"/);
});

test('svg: strips scripts, styles, foreignObject, animation and links', () => {
  const r = clean(`<svg viewBox="0 0 10 10">
    <script>alert(1)</script><script href="https://x/y.js"/>
    <style>@import url(https://evil); rect { fill: url(https://x) }</style>
    <foreignObject><div xmlns="http://www.w3.org/1999/xhtml"><iframe src="https://x"/></div></foreignObject>
    <set attributeName="href" to="javascript:alert(1)"/>
    <animate attributeName="x" values="0;1"/>
    <a href="javascript:alert(1)"><rect width="1" height="1"/></a>
    <image href="https://tracker/pixel.png"/>
    <rect width="2" height="2"/>
  </svg>`);
  for (const bad of ['script', 'style', 'foreignObject', 'iframe', '<set', '<animate', '<a ', 'javascript', 'image', 'https', '@import']) {
    assert.ok(!r.svg.includes(bad), `${bad} must be removed: ${r.svg}`);
  }
  assert.ok(r.svg.includes('<rect width="2" height="2">'));
  assert.ok(r.removed.includes('<script>') && r.removed.includes('<foreignObject>') && r.removed.includes('<set>'));
});

test('svg: strips event handlers whatever their case', () => {
  const r = clean('<svg viewBox="0 0 10 10" onload="alert(1)"><rect width="1" height="1" onClick="x()" ONMOUSEOVER="y()"/></svg>');
  assert.ok(!/on\w+=/i.test(r.svg));
  assert.ok(r.removed.includes('event handlers'));
});

test('svg: strips external and scripted references', () => {
  const r = clean(`<svg viewBox="0 0 10 10">
    <use href="https://evil/sprite.svg#a"/>
    <use xlink:href="data:image/svg+xml;base64,AAAA"/>
    <rect width="1" height="1" fill="url(https://evil/p)" stroke="url('#ok')"/>
    <rect width="1" height="1" style="fill: url(https://evil); stroke: red; stroke-width: 2"/>
    <rect width="1" height="1" style="fill: u\\72l(https://evil)"/>
    <rect width="1" height="1" style="position: fixed; top: 0; behavior: url(x.htc)"/>
    <rect width="1" height="1" class="fixed inset-0"/>
  </svg>`);
  assert.ok(!r.svg.includes('evil'));
  assert.ok(!r.svg.includes('data:'));
  assert.ok(!r.svg.includes('\\'));
  assert.ok(!r.svg.includes('position') && !r.svg.includes('behavior'));
  assert.ok(!r.svg.includes('class='));
  assert.ok(r.svg.includes('stroke="url(#t-ok)"'));
  assert.ok(r.svg.includes('style="stroke: red; stroke-width: 2"'));
});

test('svg: rejects malformed input, entities and oversize sources', () => {
  const bad: [string, RegExp][] = [
    ['<svg viewBox="0 0 1 1"><rect></svg>', /does not match|not closed/],
    ['<svg viewBox="0 0 1 1"><rect width=1/></svg>', /quoted/],
    ['<!DOCTYPE svg [<!ENTITY x "y">]><svg viewBox="0 0 1 1"/>', /DOCTYPE/],
    ['<svg><rect/></svg>', /viewBox/],
    ['<g/>', /exactly one <svg>/],
    ['<svg viewBox="0 0 1 1"/><svg viewBox="0 0 1 1"/>', /exactly one/],
    ['<svg viewBox="0 0 1 1"><rect x="1" x="2"/></svg>', /twice/],
  ];
  for (const [src, pattern] of bad) {
    const r = sanitizeSvg(src);
    assert.equal(r.ok, false, src);
    if (!r.ok) assert.match(r.error, pattern);
  }
  const big = sanitizeSvg(`<svg viewBox="0 0 1 1">${' '.repeat(SVG_MAX_CHARS)}</svg>`);
  assert.ok(!big.ok && /larger than/.test(big.error));
});

test('svg: viewBox is derived from width and height, and size is bounded', () => {
  const r = clean('<svg width="300" height="150"><rect width="10" height="10"/></svg>');
  assert.ok(r.svg.includes('viewBox="0 0 300 150"'));
  const huge = clean('<svg viewBox="0 0 100000 50000"><rect width="1" height="1"/></svg>');
  assert.ok(huge.width <= 4000 && huge.height <= 4000);
  assert.equal(huge.width / huge.height, 2);
});

test('svg: text is escaped and entities decode safely', () => {
  const r = clean('<svg viewBox="0 0 10 10"><text>F &lt; mg &amp;&#x3c;b&#62; <![CDATA[<i>]]></text></svg>');
  assert.ok(r.svg.includes('F &lt; mg &amp;&lt;b&gt;'));
  assert.ok(r.svg.includes('&lt;i&gt;'));
  assert.ok(!r.svg.includes('<i>') && !r.svg.includes('<b>'));
});

test('svg: sketch hint and context compaction', () => {
  assert.ok(wantsSketch('<!-- sketch -->\n<svg viewBox="0 0 1 1"/>'));
  assert.ok(wantsSketch('  <!--sketch--><svg/>'));
  assert.ok(!wantsSketch('<svg viewBox="0 0 1 1"><!-- sketch --></svg>'));
  const r = clean('<!-- sketch -->\n<svg viewBox="0 0 1 1"><rect width="1" height="1"/></svg>');
  assert.equal(r.sketch, true);
  const long = `<svg viewBox="0 0 1 1"><path d="${'M0 0 L1 1 '.repeat(100)}"/></svg>`;
  const compact = compactSvg(long);
  assert.ok(compact.length < long.length && compact.includes('…'));
});
