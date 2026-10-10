import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark, oneLight } from 'react-syntax-highlighter/dist/esm/styles/prism';
import type { CodeHighlightProps } from './CodeHighlight';

/** Prism highlighting; load it through `CodeHighlight`, which keeps Prism's grammars out of the startup bundle. */
export function CodeHighlightView({ code, language, dark, customStyle, showLineNumbers = false, wrapLongLines = false, preTag }: CodeHighlightProps) {
  return (
    <SyntaxHighlighter
      style={dark ? oneDark : oneLight}
      language={language}
      PreTag={preTag}
      showLineNumbers={showLineNumbers}
      wrapLongLines={wrapLongLines}
      customStyle={customStyle}
    >
      {code}
    </SyntaxHighlighter>
  );
}
