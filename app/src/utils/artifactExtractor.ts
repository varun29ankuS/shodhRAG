/**
 * Artifact Extractor Utility
 * Parses LLM responses for artifacts (code blocks, diagrams, charts, etc.)
 */

export interface Artifact {
  id: string;
  artifact_type: {
    Code?: { language: string } | null;
    Markdown?: null;
    Mermaid?: { diagram_type: string } | null;
    Chart?: null;
    Table?: null;
    SVG?: null;
    HTML?: null;
  };
  title: string;
  content: string;
  language?: string;
  editable: boolean;
  version: number;
  created_at: string;
}

// ─── Chart JSON Utilities ────────────────────────────────────────────────────

/** All chart type keywords that we recognize */
const CHART_TYPES = new Set([
  'bar', 'line', 'pie', 'doughnut', 'donut', 'area', 'radar',
  'scatter', 'bubble', 'horizontalbar', 'horizontal_bar', 'polararea',
]);

/**
 * Extract a balanced JSON object from text starting at `start` (must be '{').
 * Handles nested braces, string literals, and escape sequences.
 */
function extractBalancedBraces(text: string, start: number): string | null {
  if (text[start] !== '{') return null;
  let depth = 0;
  let inString = false;
  let escape = false;
  for (let i = start; i < text.length; i++) {
    const ch = text[i];
    if (escape) { escape = false; continue; }
    if (ch === '\\' && inString) { escape = true; continue; }
    if (ch === '"') { inString = !inString; continue; }
    if (inString) continue;
    if (ch === '{') depth++;
    if (ch === '}') { depth--; if (depth === 0) return text.slice(start, i + 1); }
  }
  return null;
}

/**
 * Clean LLM-produced "JSON" that often contains JS-isms into valid JSON.
 * Handles:
 *  - Single quotes → double quotes (outside strings)
 *  - Trailing commas before } or ]
 *  - JS arrow functions:  (val) => `${val}%`
 *  - JS function expressions: function(val) { ... }
 *  - Empty values:  "data": ,  or  "data": }
 *  - Unquoted keys:  { type: "bar" }
 *  - JS literals: true, false, null, undefined, NaN, Infinity (kept or nulled)
 *  - // and /* ... * / comments
 */
export function cleanChartJson(raw: string): string {
  let s = raw.trim();

  // Strip "chart" prefix if present
  s = s.replace(/^\s*chart\s*/i, '');

  // ── Phase 1: Strip the "options" key entirely ─────────────────────────
  // LLMs include Chart.js options with JS callbacks, formatters, etc. that
  // are impossible to safely clean. We don't use options, so remove them.
  // Match "options": { ... } at the top level by finding balanced braces.
  s = stripJsonKey(s, 'options');
  s = stripJsonKey(s, 'plugins');

  // ── Phase 2: Try parsing as-is first (most LLM output is valid JSON) ──
  {
    let attempt = s;
    attempt = attempt.replace(/,\s*([}\]])/g, '$1'); // trailing commas
    try { JSON.parse(attempt); return attempt; } catch {}
  }

  // ── Phase 3: Progressive cleaning for broken JSON ─────────────────────
  // Remove comments
  s = s.replace(/\/\/[^\n]*/g, '');
  s = s.replace(/\/\*[\s\S]*?\*\//g, '');

  // Replace JS arrow functions: (value) => `${value}%`
  s = s.replace(/:\s*\([^)]*\)\s*=>[\s\S]*?(?=[,}\]])/g, ': null');
  // Replace function expressions: function(…) { … }
  s = s.replace(/:\s*function\s*\([^)]*\)\s*\{[^}]*\}/g, ': null');
  // Replace string-wrapped functions: "function(…) { … }"
  s = s.replace(/"function\s*\([^"]*\)[\s\S]*?"/g, 'null');

  // Replace undefined / NaN / Infinity with null
  s = s.replace(/:\s*undefined\b/g, ': null');
  s = s.replace(/:\s*NaN\b/g, ': null');
  s = s.replace(/:\s*-?Infinity\b/g, ': null');

  // Fix empty values: "key": , or "key": } or "key": ]
  s = s.replace(/:\s*,/g, ': null,');
  s = s.replace(/:\s*}/g, ': null}');
  s = s.replace(/:\s*\]/g, ': null]');

  // Remove trailing commas
  s = s.replace(/,\s*([}\]])/g, '$1');

  // Try parsing before doing destructive single-quote replacement
  try { JSON.parse(s); return s; } catch {}

  // ── Phase 4: Single-quote fix (only if still not parseable) ───────────
  // Only replace single quotes used as string delimiters, not apostrophes
  // inside double-quoted strings. Strategy: process char by char.
  s = replaceSingleQuoteDelimiters(s);

  // Quote unquoted object keys: { type: → { "type":
  s = s.replace(/([{,]\s*)(\w+)\s*:/g, '$1"$2":');

  // Final trailing comma cleanup
  s = s.replace(/,\s*([}\]])/g, '$1');

  return s;
}

/**
 * Remove an entire key-value pair from a JSON-ish string by key name.
 * Handles nested objects/arrays as values via balanced-brace tracking.
 */
function stripJsonKey(json: string, key: string): string {
  // Match "key" : (possibly with various quoting)
  const pattern = new RegExp(`"${key}"\\s*:\\s*`, 'g');
  let match;
  while ((match = pattern.exec(json)) !== null) {
    const valueStart = match.index + match[0].length;
    let valueEnd: number;

    if (json[valueStart] === '{' || json[valueStart] === '[') {
      // Find balanced closing
      const closer = json[valueStart] === '{' ? '}' : ']';
      let depth = 0;
      let inStr = false;
      let esc = false;
      valueEnd = valueStart;
      for (let i = valueStart; i < json.length; i++) {
        const ch = json[i];
        if (esc) { esc = false; continue; }
        if (ch === '\\' && inStr) { esc = true; continue; }
        if (ch === '"') { inStr = !inStr; continue; }
        if (inStr) continue;
        if (ch === json[valueStart]) depth++;
        if (ch === closer) { depth--; if (depth === 0) { valueEnd = i + 1; break; } }
      }
    } else if (json[valueStart] === '"') {
      // String value — find closing quote
      let esc = false;
      valueEnd = valueStart + 1;
      for (let i = valueStart + 1; i < json.length; i++) {
        if (esc) { esc = false; continue; }
        if (json[i] === '\\') { esc = true; continue; }
        if (json[i] === '"') { valueEnd = i + 1; break; }
      }
    } else {
      // Primitive (number, bool, null) — read until , or } or ]
      const rest = json.substring(valueStart).match(/^[^,}\]]+/);
      valueEnd = valueStart + (rest ? rest[0].length : 0);
    }

    // Remove the key-value pair including leading comma or trailing comma
    let removeStart = match.index;
    let removeEnd = valueEnd;

    // Check for leading comma
    const before = json.substring(0, removeStart).trimEnd();
    if (before.endsWith(',')) {
      removeStart = before.lastIndexOf(',');
    }
    // Or trailing comma
    const after = json.substring(removeEnd).trimStart();
    if (after.startsWith(',')) {
      removeEnd += json.substring(removeEnd).indexOf(',') + 1;
    }

    json = json.substring(0, removeStart) + json.substring(removeEnd);
    pattern.lastIndex = removeStart; // reset regex position
  }
  return json;
}

/**
 * Replace single quotes used as JSON string delimiters with double quotes,
 * while preserving single quotes inside double-quoted strings (apostrophes).
 */
function replaceSingleQuoteDelimiters(s: string): string {
  const result: string[] = [];
  let inDouble = false;
  let inSingle = false;
  let escape = false;

  for (let i = 0; i < s.length; i++) {
    const ch = s[i];
    if (escape) { escape = false; result.push(ch); continue; }
    if (ch === '\\') { escape = true; result.push(ch); continue; }

    if (ch === '"' && !inSingle) {
      inDouble = !inDouble;
      result.push(ch);
    } else if (ch === "'" && !inDouble) {
      // This single quote is a string delimiter — replace with double quote
      inSingle = !inSingle;
      result.push('"');
    } else {
      result.push(ch);
    }
  }
  return result.join('');
}

/**
 * Try to parse a string as a chart spec JSON. Returns the parsed object
 * with normalized fields, or null if it doesn't look like a chart.
 */
export function tryParseChartSpec(raw: string): {
  type: string;
  title?: string;
  data: {
    labels: string[];
    datasets: { label: string; data: (number | null)[]; backgroundColor?: string | string[]; borderColor?: string }[];
  };
} | null {
  try {
    const cleaned = cleanChartJson(raw);
    const parsed = JSON.parse(cleaned);

    // Must have "type"
    const chartType = (parsed.type || '').toLowerCase().replace(/[\s_-]/g, '');
    if (!CHART_TYPES.has(chartType)) return null;

    // Must have "data" with at least labels or datasets
    if (!parsed.data || typeof parsed.data !== 'object') return null;
    if (!parsed.data.labels && !parsed.data.datasets) return null;

    // Normalize labels
    const labels: string[] = Array.isArray(parsed.data.labels)
      ? parsed.data.labels.map((l: unknown) => String(l ?? ''))
      : [];

    // Normalize datasets
    let datasets = Array.isArray(parsed.data.datasets) ? parsed.data.datasets : [];
    if (datasets.length === 0 && labels.length > 0) {
      // Some LLMs put data directly: { data: { labels: [...], values: [...] } }
      const values = parsed.data.values || parsed.data.data;
      if (Array.isArray(values)) {
        datasets = [{ data: values }];
      }
    }
    datasets = datasets.map((ds: any, i: number) => ({
      label: ds.label || ds.name || `Series ${i + 1}`,
      data: Array.isArray(ds.data) ? ds.data.map((v: unknown) => (typeof v === 'number' ? v : null)) : [],
      backgroundColor: ds.backgroundColor || ds.background || ds.color || ds.colors,
      borderColor: ds.borderColor || ds.border,
    }));

    if (labels.length === 0 && datasets.length > 0 && datasets[0].data.length === 0) {
      return null; // No actual data
    }

    return {
      type: chartType,
      title: parsed.title || parsed.name || undefined,
      data: { labels, datasets },
    };
  } catch {
    return null;
  }
}

// ─── Main Extraction ─────────────────────────────────────────────────────────

/**
 * Extract artifacts from streaming LLM response.
 * Supports:
 *   - <artifact> XML tags
 *   - Inline chart JSON (with/without "chart" prefix, with/without code fences)
 *   - ```chart, ```json (auto-detect chart), ```mermaid, etc. code blocks
 *   - Generic code blocks
 */
export function extractArtifacts(text: string): Artifact[] {
  const artifacts: Artifact[] = [];
  const uid = () => `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;

  // ── Pattern 1: <artifact> tags ──────────────────────────────────────────
  const artifactTagRegex = /<artifact\s+(?:id="([^"]+)")?\s*(?:type="([^"]+)")?\s*(?:language="([^"]+)")?\s*(?:title="([^"]+)")?\s*>([\s\S]*?)<\/artifact>/gi;

  let match;
  while ((match = artifactTagRegex.exec(text)) !== null) {
    const [, id, type, language, title, content] = match;
    artifacts.push({
      id: id || `artifact-${uid()}`,
      artifact_type: determineArtifactType(type, language, content),
      title: title || 'Untitled Artifact',
      content: content.trim(),
      language: language,
      editable: true,
      version: 1,
      created_at: new Date().toISOString(),
    });
  }
  if (artifacts.length > 0) return artifacts;

  // ── Pattern 2: Code-fenced blocks ──────────────────────────────────────
  // Process these FIRST so we can also detect ```json blocks that are charts
  const codeBlockRegex = /```(\w+)?\n([\s\S]*?)```/g;
  const fencedChartRanges: [number, number][] = []; // track which ranges are chart fences

  while ((match = codeBlockRegex.exec(text)) !== null) {
    const [fullMatch, language, content] = match;
    const lowerLang = (language || '').toLowerCase();

    // Check if this code block IS a chart (```chart or ```json with chart content)
    const isExplicitChart = lowerLang === 'chart';
    const isJsonChart = (lowerLang === 'json' || lowerLang === '') && tryParseChartSpec(content) !== null;

    if (isExplicitChart || isJsonChart) {
      const spec = tryParseChartSpec(content);
      artifacts.push({
        id: `chart-${uid()}`,
        artifact_type: { Chart: null },
        title: spec?.title || 'Chart',
        content: cleanChartJson(content),
        editable: false,
        version: 1,
        created_at: new Date().toISOString(),
      });
      fencedChartRanges.push([match.index, match.index + fullMatch.length]);
      continue;
    }

    // Skip very short code blocks (< 3 lines)
    if (content.trim().split('\n').length < 3) continue;

    // Detect special block types
    const isMermaid = lowerLang === 'mermaid' || /^(graph|flowchart|sequenceDiagram|classDiagram|stateDiagram|erDiagram|journey|gantt|pie|gitGraph)/i.test(content.trim());
    const isTable = lowerLang === 'table';

    const artifactType = isTable
      ? { Table: null }
      : isMermaid
      ? { Mermaid: { diagram_type: detectMermaidType(content) } }
      : { Code: { language: language || 'plaintext' } };

    const title = isTable
      ? 'Table'
      : generateTitle(language, content, isMermaid);

    artifacts.push({
      id: `code-${uid()}`,
      artifact_type: artifactType,
      title,
      content: content.trim(),
      language: language,
      editable: true,
      version: 1,
      created_at: new Date().toISOString(),
    });
  }

  // If we already found fenced charts or other artifacts, return them
  if (artifacts.length > 0) return artifacts;

  // ── Pattern 3: Inline chart JSON (no code fences) ─────────────────────
  // Strip code-fenced content so we don't double-detect
  const textWithoutFences = text.replace(/```[\s\S]*?```/g, '');

  for (let i = 0; i < textWithoutFences.length; i++) {
    // Look for "chart" keyword followed by {, or standalone {
    const isChartKeyword = textWithoutFences.substring(i, i + 10).match(/^chart\s*\{/i);
    const isBrace = textWithoutFences[i] === '{';

    if (!isChartKeyword && !isBrace) continue;

    const braceStart = isChartKeyword
      ? textWithoutFences.indexOf('{', i)
      : i;
    if (braceStart === -1) continue;

    const jsonStr = extractBalancedBraces(textWithoutFences, braceStart);
    if (!jsonStr || jsonStr.length < 20) continue;

    // Quick pre-check: must have "type" and "data" somewhere
    if (!jsonStr.includes('type') || !jsonStr.includes('data')) continue;

    const spec = tryParseChartSpec(jsonStr);
    if (spec) {
      const startPos = isChartKeyword ? i : braceStart;
      artifacts.push({
        id: `chart-${uid()}`,
        artifact_type: { Chart: null },
        title: spec.title || 'Chart',
        content: cleanChartJson(jsonStr),
        editable: false,
        version: 1,
        created_at: new Date().toISOString(),
      });
      // Skip past this block
      i = braceStart + jsonStr.length - 1;
    }
  }

  return artifacts;
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function determineArtifactType(type?: string, language?: string, content?: string): Artifact['artifact_type'] {
  if (!type && !language) {
    if (content && /^(graph|flowchart|sequenceDiagram|classDiagram)/i.test(content.trim())) {
      return { Mermaid: { diagram_type: detectMermaidType(content) } };
    }
    return { Code: { language: 'plaintext' } };
  }

  const lowerType = (type || language || '').toLowerCase();

  if (lowerType === 'chart') return { Chart: null };
  if (lowerType === 'table') return { Table: null };
  if (lowerType === 'mermaid' || lowerType === 'diagram') return { Mermaid: { diagram_type: detectMermaidType(content || '') } };
  if (lowerType === 'markdown' || lowerType === 'md') return { Markdown: null };
  if (lowerType === 'svg') return { SVG: null };
  if (lowerType === 'html') return { HTML: null };

  return { Code: { language: language || 'plaintext' } };
}

function detectMermaidType(content: string): string {
  const firstLine = content.trim().split('\n')[0].toLowerCase();
  if (firstLine.startsWith('graph')) return 'graph';
  if (firstLine.startsWith('flowchart')) return 'flowchart';
  if (firstLine.startsWith('sequencediagram')) return 'sequence';
  if (firstLine.startsWith('classdiagram')) return 'class';
  if (firstLine.startsWith('statediagram')) return 'state';
  if (firstLine.startsWith('erdiagram')) return 'er';
  if (firstLine.startsWith('journey')) return 'journey';
  if (firstLine.startsWith('gantt')) return 'gantt';
  if (firstLine.startsWith('pie')) return 'pie';
  if (firstLine.startsWith('gitgraph')) return 'gitGraph';
  return 'flowchart';
}

function generateTitle(language?: string, content?: string, isMermaid?: boolean): string {
  if (isMermaid) {
    const type = detectMermaidType(content || '');
    return `${type.charAt(0).toUpperCase() + type.slice(1)} Diagram`;
  }
  if (!language || language === 'plaintext') return 'Code Snippet';

  const lines = (content || '').trim().split('\n').slice(0, 5);
  for (const line of lines) {
    const funcMatch = line.match(/(?:fn|function|def|func|const|let|var)\s+(\w+)/);
    if (funcMatch) return `${language.toUpperCase()}: ${funcMatch[1]}()`;
    const classMatch = line.match(/(?:class|struct|interface|type)\s+(\w+)/);
    if (classMatch) return `${language.toUpperCase()}: ${classMatch[1]}`;
  }
  return `${language.charAt(0).toUpperCase() + language.slice(1)} Code`;
}

// ─── Public Utilities ────────────────────────────────────────────────────────

/**
 * Quick check: does this text likely contain extractable artifacts?
 */
export function hasArtifacts(text: string): boolean {
  if (text.includes('<artifact')) return true;
  if (/```\w+\n[\s\S]{50,}```/.test(text)) return true;
  // Inline chart patterns
  if (/\bchart\s*\{/i.test(text)) return true;
  if (/"type"\s*:\s*"(bar|line|pie|doughnut|donut|area|radar|scatter|bubble|horizontalbar|horizontal_bar|polararea)"/i.test(text) && /"data"/i.test(text)) return true;
  // Unquoted or single-quoted type keys
  if (/['"]?type['"]?\s*:\s*['"]?(bar|line|pie|doughnut|donut|area|radar)['"]?/i.test(text) && /['"]?data['"]?\s*:/i.test(text)) return true;
  return false;
}

/**
 * Strip chart content from display text (returns text with chart JSON removed).
 * Used by the message renderer to avoid showing raw chart JSON inline.
 */
export function stripChartContent(text: string): string {
  let result = text;

  // Strip ```chart and ```json code blocks that are charts
  result = result.replace(/```(?:chart|json)\s*\n([\s\S]*?)```/g, (fullMatch, content) => {
    const spec = tryParseChartSpec(content);
    return spec ? '' : fullMatch;
  });

  // Strip inline chart JSON (with or without "chart" prefix)
  // Work on text outside code fences
  const codeBlocks: string[] = [];
  result = result.replace(/```[\s\S]*?```/g, (m) => { codeBlocks.push(m); return `\x01CB${codeBlocks.length - 1}\x01`; });

  const ranges: [number, number][] = [];
  for (let i = 0; i < result.length; i++) {
    const isChartKeyword = result.substring(i, i + 10).match(/^chart\s*\{/i);
    const isBrace = result[i] === '{';
    if (!isChartKeyword && !isBrace) continue;

    const braceStart = isChartKeyword ? result.indexOf('{', i) : i;
    if (braceStart === -1) continue;

    const jsonStr = extractBalancedBraces(result, braceStart);
    if (!jsonStr || jsonStr.length < 20) continue;
    if (!jsonStr.includes('type') || !jsonStr.includes('data')) continue;

    const spec = tryParseChartSpec(jsonStr);
    if (spec) {
      const startPos = isChartKeyword ? i : braceStart;
      ranges.push([startPos, braceStart + jsonStr.length]);
      i = braceStart + jsonStr.length - 1;
    }
  }

  // Remove ranges from end to start
  for (let ri = ranges.length - 1; ri >= 0; ri--) {
    result = result.slice(0, ranges[ri][0]) + result.slice(ranges[ri][1]);
  }

  // Restore code blocks
  result = result.replace(/\x01CB(\d+)\x01/g, (_, idx) => codeBlocks[parseInt(idx)]);

  return result;
}

/**
 * Incremental artifact extraction for streaming responses.
 */
export class StreamingArtifactExtractor {
  private previousText = '';
  private extractedIds = new Set<string>();

  extractNew(currentText: string): Artifact[] {
    if (currentText === this.previousText) return [];

    const allArtifacts = extractArtifacts(currentText);
    const newArtifacts = allArtifacts.filter(artifact => {
      if (this.extractedIds.has(artifact.id)) return false;
      this.extractedIds.add(artifact.id);
      return true;
    });

    this.previousText = currentText;
    return newArtifacts;
  }

  reset() {
    this.previousText = '';
    this.extractedIds.clear();
  }
}
