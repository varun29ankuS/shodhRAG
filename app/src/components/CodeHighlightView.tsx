import { useMemo } from 'react';
import type { ReactElement } from 'react';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import type { SyntaxHighlighterProps } from 'react-syntax-highlighter';
import { oneDark, oneLight } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { LruCache } from '../features/ask/viewer/lruCache';
import type { CodeHighlightProps } from './CodeHighlight';

/**
 * Highlighted blocks by content and options. Answers in a long conversation
 * mount and unmount as it scrolls; tokenizing a block again on every mount
 * was the largest cost of scrolling back over code.
 */
const highlighted = new LruCache<ReactElement>({
  maxEntries: 128,
  // Declared size: the code's length; a highlighted tree is a small multiple of it.
  maxBytes: 2_000_000,
  dispose: () => undefined,
});

/**
 * The highlighter is a plain function of its props (no hooks, no state), so
 * the element tree it returns can be kept and mounted again unchanged. Its
 * typings declare a class; a version that really is one is rendered as a
 * component, uncached.
 */
const renderHighlighter: ((props: SyntaxHighlighterProps) => ReactElement) | null =
  typeof SyntaxHighlighter === 'function' && !(SyntaxHighlighter.prototype && 'isReactComponent' in SyntaxHighlighter.prototype)
    ? (SyntaxHighlighter as unknown as (props: SyntaxHighlighterProps) => ReactElement)
    : null;

/** Prism highlighting; load it through `CodeHighlight`, which keeps Prism's grammars out of the startup bundle. */
export function CodeHighlightView({ code, language, dark, customStyle, showLineNumbers = false, wrapLongLines = false, preTag }: CodeHighlightProps) {
  const styleKey = customStyle ? JSON.stringify(customStyle) : '';
  return useMemo(() => {
    const props: SyntaxHighlighterProps = {
      style: dark ? oneDark : oneLight,
      language,
      PreTag: preTag,
      showLineNumbers,
      wrapLongLines,
      customStyle,
      children: code,
    };
    if (!renderHighlighter) return <SyntaxHighlighter {...props} />;
    const key = [dark ? 'd' : 'l', language, preTag ?? 'pre', showLineNumbers ? 1 : 0, wrapLongLines ? 1 : 0, styleKey, code].join('\u0001');
    const cached = highlighted.get(key);
    if (cached) return cached;
    const tree = renderHighlighter(props);
    highlighted.set(key, tree, code.length);
    return tree;
    // styleKey stands for customStyle's content.
  }, [code, language, dark, preTag, showLineNumbers, wrapLongLines, styleKey]);
}
