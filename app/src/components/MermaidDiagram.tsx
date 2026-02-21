import React, { useEffect, useRef } from 'react';

interface MermaidDiagramProps {
  chart: string;
  className?: string;
}

const MermaidDiagram: React.FC<MermaidDiagramProps> = ({ chart, className = '' }) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = React.useState<string | null>(null);

  useEffect(() => {
    const renderDiagram = async () => {
      if (!containerRef.current || !chart) return;

      try {
        // Dynamically import mermaid
        const mermaid = (await import('mermaid')).default;

        // Initialize mermaid with configuration
        mermaid.initialize({
          startOnLoad: false,
          theme: 'default',
          securityLevel: 'loose',
          flowchart: {
            useMaxWidth: true,
            htmlLabels: true,
            curve: 'basis',
          },
          sequence: {
            diagramMarginX: 50,
            diagramMarginY: 10,
            boxTextMargin: 5,
            noteMargin: 10,
            messageMargin: 35,
            mirrorActors: true,
          },
        });

        // Clear previous content
        containerRef.current.innerHTML = '';

        // Generate unique ID
        const id = `mermaid-${Math.random().toString(36).substr(2, 9)}`;

        // Create diagram element
        const element = document.createElement('div');
        element.className = 'mermaid-content';
        containerRef.current.appendChild(element);

        // Render the diagram
        const { svg } = await mermaid.render(id, chart);
        element.innerHTML = svg;

        setError(null);
      } catch (err) {
        console.error('Mermaid rendering error:', err);
        setError(err instanceof Error ? err.message : 'Failed to render diagram');
      }
    };

    renderDiagram();
  }, [chart]);

  if (error) {
    return (
      <div className={`p-4 bg-red-50 border-2 border-red-200 rounded-lg ${className}`}>
        <p className="text-red-800 font-semibold">Failed to render diagram</p>
        <p className="text-red-600 text-sm mt-2">{error}</p>
        <details className="mt-2">
          <summary className="cursor-pointer text-red-700 text-sm">Show diagram code</summary>
          <pre className="mt-2 text-xs bg-red-100 p-2 rounded overflow-x-auto">{chart}</pre>
        </details>
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      className={`mermaid-container bg-white p-4 rounded-lg overflow-auto ${className}`}
      style={{ minHeight: '200px' }}
    />
  );
};

export default MermaidDiagram;
