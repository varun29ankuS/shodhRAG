import { useEffect, useRef } from 'react';
import Editor from '@monaco-editor/react';
import type { Artifact } from './EnhancedArtifactPanel';

interface CodeArtifactProps {
  artifact: Artifact;
  theme?: string;
}

export function CodeArtifact({
  artifact,
  theme = 'light',
}: CodeArtifactProps) {
  const getLanguage = (): string => {
    if (artifact.artifact_type.Code) {
      const lang = artifact.language || 'plaintext';
      // Map common language names to Monaco language IDs
      const langMap: Record<string, string> = {
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
        'sh': 'shell',
        'bash': 'shell',
        'zsh': 'shell',
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
      };
      return langMap[lang.toLowerCase()] || lang;
    }
    return 'plaintext';
  };

  const monacoTheme = theme === 'dark' ? 'vs-dark' : 'light';

  return (
    <div className="h-full flex flex-col bg-white dark:bg-gray-900">
      {/* Monaco Editor */}
      <div className="flex-1 overflow-hidden">
        <Editor
          height="100%"
          language={getLanguage()}
          value={artifact.content}
          theme={monacoTheme}
          options={{
            readOnly: true,
            minimap: { enabled: true },
            fontSize: 13,
            lineNumbers: 'on',
            scrollBeyondLastLine: false,
            automaticLayout: true,
            tabSize: 2,
            wordWrap: 'on',
            padding: { top: 16, bottom: 16 },
            lineHeight: 20,
            fontFamily: "'Fira Code', 'Cascadia Code', Consolas, monospace",
            fontLigatures: true,
            quickSuggestions: false,
            renderWhitespace: 'selection',
            smoothScrolling: true,
            cursorBlinking: 'smooth',
          }}
          loading={
            <div className="flex items-center justify-center h-full">
              <div className="text-sm text-gray-500 dark:text-gray-400">
                Loading editor...
              </div>
            </div>
          }
        />
      </div>
    </div>
  );
}
