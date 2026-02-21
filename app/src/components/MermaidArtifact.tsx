import { useEffect, useRef, useState } from 'react';
import mermaid from 'mermaid';
import type { Artifact } from './EnhancedArtifactPanel';

interface MermaidArtifactProps {
  artifact: Artifact;
}

export function MermaidArtifact({
  artifact,
}: MermaidArtifactProps) {
  const mermaidRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    mermaid.initialize({
      startOnLoad: false,
      theme: 'default',
      securityLevel: 'loose',
      fontFamily: 'system-ui, sans-serif',
    });
  }, []);

  useEffect(() => {
    if (mermaidRef.current) {
      renderMermaid();
    }
  }, [artifact.content]);

  const renderMermaid = async () => {
    if (!mermaidRef.current) return;

    try {
      setError(null);
      mermaidRef.current.innerHTML = '';

      // Ensure content has a diagram type if missing
      let diagramContent = artifact.content.trim();

      console.log('🎨 Rendering mermaid diagram...');
      console.log('   Content length:', diagramContent.length);
      console.log('   Content preview:', diagramContent.substring(0, 100));

      // If content doesn't start with a diagram type, try to detect or add one
      const hasType = /^(graph|flowchart|sequenceDiagram|classDiagram|stateDiagram|erDiagram|journey|gantt|pie|gitGraph)/.test(diagramContent);

      if (!hasType) {
        // Default to flowchart if no type detected
        console.log('⚠️  No diagram type detected, defaulting to flowchart TD');
        diagramContent = `flowchart TD\n${diagramContent}`;
      } else {
        console.log('✅ Diagram type detected:', diagramContent.split('\n')[0]);
      }

      const { svg } = await mermaid.render(
        `mermaid-${artifact.id}-${Date.now()}`,
        diagramContent
      );

      console.log('✅ Mermaid rendered successfully, SVG length:', svg.length);
      mermaidRef.current.innerHTML = svg;
    } catch (err: any) {
      console.error('❌ Mermaid rendering error:', err);
      console.error('   Error message:', err?.message);
      console.error('   Error details:', err);
      setError(err?.message || 'Failed to render diagram');
    }
  };

  return (
    <div className="h-full flex flex-col bg-white dark:bg-gray-900">
      {/* Content */}
      <div className="flex-1 overflow-auto p-6 flex items-center justify-center">
        {error ? (
          <div className="p-4 rounded-lg border border-red-500 bg-red-50 dark:bg-red-950">
            <p className="text-sm font-semibold mb-1 text-red-700 dark:text-red-300">Diagram Error</p>
            <p className="text-xs text-red-600 dark:text-red-400">{error}</p>
          </div>
        ) : (
          <div
            ref={mermaidRef}
            className="mermaid-diagram w-full"
          />
        )}
      </div>
    </div>
  );
}
