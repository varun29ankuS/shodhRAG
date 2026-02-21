import { useEffect, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Artifact } from './EnhancedArtifactPanel';

interface MarkdownArtifactProps {
  artifact: Artifact;
}

export function MarkdownArtifact({
  artifact,
}: MarkdownArtifactProps) {

  return (
    <div className="h-full flex flex-col bg-white dark:bg-gray-900">
      {/* Content */}
      <div className="flex-1 overflow-auto p-6">
        <div className="prose prose-sm dark:prose-invert max-w-none">
          <ReactMarkdown>{artifact.content}</ReactMarkdown>
        </div>
      </div>
    </div>
  );
}
