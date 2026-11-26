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
    <div className="h-full flex flex-col bg-shodh-surface">
      {/* Content */}
      <div className="flex-1 overflow-auto p-6 flex items-center justify-center">
        {error ? (
          <div className="p-4 rounded-lg border border-shodh-error/40 bg-shodh-error/10">
            <p className="text-body font-semibold mb-1 text-shodh-text">Diagram Error</p>
            <p className="text-caption text-shodh-error">{error}</p>
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
