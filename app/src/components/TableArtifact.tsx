import type { Artifact } from './EnhancedArtifactPanel';

interface TableArtifactProps {
  artifact: Artifact;
  theme: string;
}

/**
 * Renders table artifacts. Parses markdown pipe-table format into a styled HTML table.
 * Falls back to raw preformatted text if parsing fails.
 */
export function TableArtifact({ artifact }: TableArtifactProps) {
  const { headers, rows } = parseMarkdownTable(artifact.content);

  if (headers.length === 0) {
    // Fallback: render as preformatted text
    return (
      <div className="h-full flex flex-col bg-shodh-surface">
        <div className="flex-1 overflow-auto p-6">
          <pre className="text-body font-mono whitespace-pre-wrap text-shodh-text">
            {artifact.content}
          </pre>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col bg-shodh-surface">
      <div className="flex-1 overflow-auto p-4">
        <table className="w-full border-collapse text-body">
          <thead>
            <tr>
              {headers.map((header, i) => (
                <th
                  key={i}
                  className="px-4 py-3 text-left font-semibold border-b-2"
                  style={{
                    borderColor: 'var(--c-border-strong)',
                    backgroundColor: 'var(--c-raised)',
                    color: 'var(--c-text)',
                  }}
                >
                  {header}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, rowIdx) => (
              <tr
                key={rowIdx}
                style={{
                  backgroundColor: rowIdx % 2 === 0 ? 'var(--c-surface)' : 'var(--c-surface-2)',
                }}
              >
                {row.map((cell, cellIdx) => (
                  <td
                    key={cellIdx}
                    className="px-4 py-2.5 border-b"
                    style={{
                      borderColor: 'var(--c-border-subtle)',
                      color: 'var(--c-text-secondary)',
                    }}
                  >
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/**
 * Parse markdown pipe-table format into headers and rows.
 * Handles tables with or without the separator row (---|---|---).
 */
function parseMarkdownTable(content: string): { headers: string[]; rows: string[][] } {
  const lines = content.trim().split('\n').filter(l => l.trim().length > 0);

  if (lines.length < 2) {
    return { headers: [], rows: [] };
  }

  const parseLine = (line: string): string[] =>
    line
      .replace(/^\|/, '')
      .replace(/\|$/, '')
      .split('|')
      .map(cell => cell.trim());

  // First line = headers
  const headers = parseLine(lines[0]);
  if (headers.length === 0) {
    return { headers: [], rows: [] };
  }

  // Skip separator row if present (contains only dashes, colons, pipes, spaces)
  let dataStart = 1;
  if (lines.length > 1 && /^[\s|:-]+$/.test(lines[1])) {
    dataStart = 2;
  }

  const rows = lines.slice(dataStart).map(line => {
    const cells = parseLine(line);
    // Pad or trim to match header count
    while (cells.length < headers.length) cells.push('');
    return cells.slice(0, headers.length);
  });

  return { headers, rows };
}
