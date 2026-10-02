import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark, oneLight } from 'react-syntax-highlighter/dist/esm/styles/prism';
import type { Artifact } from './EnhancedArtifactPanel';
import { getArtifactKind, getArtifactCodeLanguage } from '../utils/artifactKind';

interface CodeArtifactProps {
  artifact: Artifact;
  theme?: string;
}

// Map common language names to Prism language ids.
const LANGUAGE_MAP: Record<string, string> = {
  'js': 'javascript',
  'ts': 'typescript',
  'py': 'python',
  'rs': 'rust',
  'rb': 'ruby',
  'go': 'go',
  'c': 'c',
  'cpp': 'cpp',
  'java': 'java',
  'cs': 'csharp',
  'php': 'php',
  'swift': 'swift',
  'kt': 'kotlin',
  'scala': 'scala',
  'r': 'r',
  'sql': 'sql',
  'sh': 'bash',
  'bash': 'bash',
  'zsh': 'bash',
  'shell': 'bash',
  'yaml': 'yaml',
  'yml': 'yaml',
  'json': 'json',
  'xml': 'xml',
  'html': 'html',
  'css': 'css',
  'scss': 'scss',
  'less': 'less',
  'md': 'markdown',
  'markdown': 'markdown',
  'plaintext': 'text',
};

/**
 * Read-only code view. Rendered with the bundled Prism highlighter so it works
 * under the app's CSP (no runtime script loading from a CDN).
 */
export function CodeArtifact({
  artifact,
  theme = 'light',
}: CodeArtifactProps) {
  const getLanguage = (): string => {
    if (getArtifactKind(artifact.artifact_type) === 'code') {
      const lang = (getArtifactCodeLanguage(artifact) || 'text').toLowerCase();
      return LANGUAGE_MAP[lang] || lang;
    }
    return 'text';
  };

  const isDark = theme === 'dark';

  return (
    <div className="h-full flex flex-col bg-white dark:bg-gray-900">
      <div className="flex-1 overflow-auto">
        <SyntaxHighlighter
          language={getLanguage()}
          style={isDark ? oneDark : oneLight}
          showLineNumbers
          wrapLongLines
          customStyle={{
            margin: 0,
            padding: '16px',
            minHeight: '100%',
            fontSize: '13px',
            lineHeight: '20px',
            fontFamily: "'Fira Code', 'Cascadia Code', Consolas, monospace",
          }}
        >
          {artifact.content}
        </SyntaxHighlighter>
      </div>
    </div>
  );
}
