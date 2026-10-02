/** Compact durations for step lines: "320ms", "1.2s", "42s", "1m 05s". */
export function formatMs(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return '0ms';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const seconds = ms / 1000;
  if (seconds < 10) return `${seconds.toFixed(1)}s`;
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const whole = Math.round(seconds);
  return `${Math.floor(whole / 60)}m ${String(whole % 60).padStart(2, '0')}s`;
}

/** "950", "3.7k", "1.2M". */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '0';
  if (n < 1000) return String(Math.round(n));
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

/** "$0.0042", "$0.12", "$3.40"; null when there was no cost. */
export function formatCost(usd: number): string | null {
  if (!Number.isFinite(usd) || usd <= 0) return null;
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(2)}`;
}

/** The model id without its provider prefix: "anthropic/claude-x" → "claude-x". */
export function shortModel(model: string): string {
  const slash = model.indexOf('/');
  return slash >= 0 ? model.slice(slash + 1) : model;
}

/** Pretty JSON for the monospace detail view, capped for very large values. */
export function prettyJson(value: unknown, maxChars = 4000): string {
  let text: string;
  try {
    text = JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    text = String(value);
  }
  return text.length > maxChars ? `${text.slice(0, maxChars)}\n…` : text;
}
