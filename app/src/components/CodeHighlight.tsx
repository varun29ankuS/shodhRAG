import { Suspense, lazy } from 'react';
import type { CSSProperties } from 'react';

export interface CodeHighlightProps {
  code: string;
  language: string;
  dark: boolean;
  customStyle?: CSSProperties;
  showLineNumbers?: boolean;
  wrapLongLines?: boolean;
  /** Element wrapping the code (`pre` unless given). */
  preTag?: 'div' | 'pre';
}

// Prism with every grammar is one of the largest libraries in the app; it
// loads with the first code block instead of at startup.
const CodeHighlightView = lazy(() => import('./CodeHighlightView').then(m => ({ default: m.CodeHighlightView })));

/** The theme's block style (one-dark / one-light `pre`), so the plain code has the highlighted block's metrics. */
const PLAIN_STYLE: CSSProperties = {
  fontFamily: '"Fira Code", "Fira Mono", Menlo, Consolas, "DejaVu Sans Mono", monospace',
  lineHeight: 1.5,
  tabSize: 2,
  padding: '1em',
  margin: '0.5em 0',
  overflow: 'auto',
};

/** The code as plain text in the same box while highlighting loads. */
function PlainCode({ code, customStyle, wrapLongLines = false, preTag = 'pre' }: CodeHighlightProps) {
  const Tag = preTag;
  return (
    <Tag style={{ ...PLAIN_STYLE, whiteSpace: wrapLongLines ? 'pre-wrap' : 'pre', ...customStyle }}>
      <code>{code}</code>
    </Tag>
  );
}

/** Syntax-highlighted code (Prism, one-dark / one-light). */
export function CodeHighlight(props: CodeHighlightProps) {
  return (
    <Suspense fallback={<PlainCode {...props} />}>
      <CodeHighlightView {...props} />
    </Suspense>
  );
}
