import React, { useEffect, useRef, useMemo } from 'react';
import { BarChart, Bar, LineChart, Line, PieChart, Pie, Cell, ScatterChart, Scatter, RadarChart, Radar, PolarGrid, PolarAngleAxis, PolarRadiusAxis, XAxis, YAxis, ZAxis, CartesianGrid, Tooltip, Legend, ResponsiveContainer } from 'recharts';
import { parseResponseWithCitations } from '../utils/citationParser';
import mermaid from 'mermaid';

// Type definitions matching Rust backend
interface StructuredOutput {
  type: 'text' | 'table' | 'chart' | 'diagram' | 'form' | 'system_action';
  content?: string;
  headers?: string[];
  rows?: string[][];
  caption?: string;
  chartType?: 'bar' | 'line' | 'pie' | 'scatter' | 'area' | 'radar' | 'doughnut' | 'bubble';
  diagramType?: 'flowchart' | 'sequence' | 'class' | 'er' | 'state' | 'gantt' | 'git' | 'journey';
  title?: string;
  data?: ChartData;
  mermaid?: string;
  description?: string;
  fields?: FormField[];
  action?: SystemAction;
}

interface ChartData {
  labels: string[];
  datasets: Dataset[];
}

interface Dataset {
  label: string;
  data: number[];
  backgroundColor?: string[];
  borderColor?: string[];
}

interface FormField {
  id: string;
  type: string;
  label: string;
  required: boolean;
  placeholder?: string;
  options?: string[];
  defaultValue?: string;
}

interface SystemAction {
  FileSystem?: any;
  Command?: any;
}

const COLORS = ['#FF6B6B', '#4ECDC4', '#45B7D1', '#FFA07A', '#98D8C8', '#F7DC6F', '#BB8FCE', '#85C1E2'];

// Component to render text with citation support
function TextOutputWithCitations({ content, searchResults, onFollowUpQuery, onOpenUrl }: {
  content: string;
  searchResults?: any[];
  onFollowUpQuery?: (query: string) => void;
  onOpenUrl?: (url: string) => void;
}) {
  // Format text content with proper paragraphs and formatting
  const formatTextContent = (text: string) => {
    // Split by double newlines for paragraphs
    const paragraphs = text.split(/\n\n+/);

    return paragraphs.map((para, pIdx) => {
      // Check if this is a bullet list (lines starting with -, *, or •)
      const lines = para.split('\n');
      const isBulletList = lines.every(line =>
        line.trim() === '' || /^[\s]*[-*•]\s/.test(line)
      );

      if (isBulletList) {
        return (
          <ul key={pIdx} className="my-3 space-y-1">
            {lines.filter(line => line.trim() !== '').map((line, lIdx) => (
              <li key={lIdx} className="text-gray-800 dark:text-gray-200">
                {line.replace(/^[\s]*[-*•]\s+/, '')}
              </li>
            ))}
          </ul>
        );
      }

      // Check if this is a numbered list
      const isNumberedList = lines.every(line =>
        line.trim() === '' || /^[\s]*\d+\.\s/.test(line)
      );

      if (isNumberedList) {
        return (
          <ol key={pIdx} className="my-3 space-y-1 list-decimal list-inside">
            {lines.filter(line => line.trim() !== '').map((line, lIdx) => (
              <li key={lIdx} className="text-gray-800 dark:text-gray-200">
                {line.replace(/^[\s]*\d+\.\s+/, '')}
              </li>
            ))}
          </ol>
        );
      }

      // Regular paragraph - preserve single line breaks within paragraphs
      return (
        <p key={pIdx} className="my-3 text-gray-800 dark:text-gray-200 leading-relaxed">
          {lines.map((line, lIdx) => (
            <React.Fragment key={lIdx}>
              {line}
              {lIdx < lines.length - 1 && <br />}
            </React.Fragment>
          ))}
        </p>
      );
    });
  };

  return (
    <div className="prose dark:prose-invert max-w-none">
      {searchResults && searchResults.length > 0 ? (
        // Parse citations and format each text segment
        parseResponseWithCitations(content, searchResults, onFollowUpQuery || (() => {}), onOpenUrl).map((segment, idx) => {
          if (typeof segment === 'string') {
            return <React.Fragment key={idx}>{formatTextContent(segment)}</React.Fragment>;
          }
          return <React.Fragment key={idx}>{segment}</React.Fragment>;
        })
      ) : (
        formatTextContent(content)
      )}
    </div>
  );
}

export function StructuredOutputRenderer({ outputs, searchResults, onFollowUpQuery, onOpenUrl }: {
  outputs: StructuredOutput[];
  searchResults?: any[];
  onFollowUpQuery?: (query: string) => void;
  onOpenUrl?: (url: string) => void;
}) {
  return (
    <div className="structured-outputs space-y-4">
      {outputs.map((output, idx) => (
        <div key={idx}>
          {output.type === 'text' && (
            <TextOutputWithCitations
              content={output.content || ''}
              searchResults={searchResults}
              onFollowUpQuery={onFollowUpQuery}
              onOpenUrl={onOpenUrl}
            />
          )}

          {output.type === 'table' && (
            <TableOutput
              headers={output.headers || []}
              rows={output.rows || []}
              caption={output.caption}
            />
          )}

          {output.type === 'chart' && output.chartType && output.data && (
            <ChartOutput
              chartType={output.chartType}
              title={output.title || ''}
              data={output.data}
              description={output.description}
            />
          )}

          {output.type === 'diagram' && output.mermaid && (
            <DiagramOutput
              diagramType={output.diagramType}
              title={output.title || ''}
              mermaid={output.mermaid}
              description={output.description}
            />
          )}

          {output.type === 'form' && (
            <FormOutput
              title={output.title || ''}
              description={output.description}
              fields={output.fields || []}
            />
          )}

          {output.type === 'system_action' && (
            <SystemActionOutput action={output.action} />
          )}
        </div>
      ))}
    </div>
  );
}

function TableOutput({ headers, rows, caption }: { headers: string[]; rows: string[][]; caption?: string }) {
  return (
    <div className="overflow-x-auto my-4 animate-fade-in-up">
      {caption && <p className="text-sm text-gray-600 dark:text-gray-400 mb-2 animate-fade-in">{caption}</p>}
      <table className="min-w-full divide-y divide-gray-200 dark:divide-gray-700 border border-gray-200 dark:border-gray-700 rounded-lg overflow-hidden">
        <thead className="bg-gradient-to-r from-shodh-red/10 to-shodh-orange/10">
          <tr>
            {headers.map((header, idx) => (
              <th
                key={idx}
                className="px-4 py-3 text-left text-xs font-semibold text-gray-700 dark:text-gray-300 uppercase tracking-wider"
              >
                {header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="bg-white dark:bg-gray-800 divide-y divide-gray-200 dark:divide-gray-700">
          {rows.map((row, rowIdx) => (
            <tr key={rowIdx} className="hover:bg-gray-50 dark:hover:bg-gray-700/50 transition-all duration-200 hover:scale-[1.01]">
              {row.map((cell, cellIdx) => (
                <td key={cellIdx} className="px-4 py-3 text-sm text-gray-900 dark:text-gray-100">
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ChartOutput({ chartType, title, data, description }: {
  chartType: string;
  title: string;
  data: ChartData;
  description?: string;
}) {
  const [selectedDataset, setSelectedDataset] = React.useState<string | null>(null);
  const [insights, setInsights] = React.useState<string | null>(null);
  const [loadingInsights, setLoadingInsights] = React.useState(false);
  const chartContainerRef = React.useRef<HTMLDivElement>(null);

  // Memoize chart data to prevent infinite re-render loops with ResponsiveContainer
  const chartData = useMemo(() =>
    data.labels.map((label, idx) => {
      const point: any = { name: label };
      data.datasets.forEach(dataset => {
        point[dataset.label] = dataset.data[idx];
      });
      return point;
    }),
    [data.labels, data.datasets]
  );

  // Memoize filtered data
  const filteredData = useMemo(() =>
    selectedDataset
      ? chartData.map(point => ({
          name: point.name,
          [selectedDataset]: point[selectedDataset]
        }))
      : chartData,
    [chartData, selectedDataset]
  );

  // Calculate insights on mount
  React.useEffect(() => {
    generateInsights();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const generateInsights = async () => {
    setLoadingInsights(true);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const prompt = `Analyze this chart data and provide 3-4 key insights:
Title: ${title}
Type: ${chartType}
Data: ${JSON.stringify({ labels: data.labels, datasets: data.datasets })}

Provide insights about:
1. Trends (increasing/decreasing patterns)
2. Outliers (unusual values)
3. Comparisons (which is highest/lowest)
4. Recommendations (actionable next steps)

Format as bullet points with emojis.`;

      const result = await invoke('llm_generate', { prompt });
      setInsights(result as string);
    } catch (error) {
      console.error('Failed to generate insights:', error);
    } finally {
      setLoadingInsights(false);
    }
  };

  const handleLegendClick = (dataKey: string) => {
    setSelectedDataset(selectedDataset === dataKey ? null : dataKey);
  };

  const exportAsPNG = async () => {
    if (!chartContainerRef.current) return;

    try {
      const svgElement = chartContainerRef.current.querySelector('svg');
      if (!svgElement) return;

      // Get SVG dimensions
      const bbox: DOMRect = svgElement.getBoundingClientRect();
      const canvas = document.createElement('canvas');
      canvas.width = Number(bbox.width) * 2; // 2x for retina
      canvas.height = Number(bbox.height) * 2;
      const ctx = canvas.getContext('2d');
      if (!ctx) return;

      ctx.scale(2, 2);
      ctx.fillStyle = 'white';
      ctx.fillRect(0, 0, bbox.width, bbox.height);

      // Serialize SVG to string
      const svgString = new XMLSerializer().serializeToString(svgElement);
      const img = new Image();
      const svgBlob = new Blob([svgString], { type: 'image/svg+xml;charset=utf-8' });
      const url = URL.createObjectURL(svgBlob);

      img.onload = () => {
        ctx.drawImage(img, 0, 0);
        URL.revokeObjectURL(url);

        canvas.toBlob((blob) => {
          if (!blob) return;
          const link = document.createElement('a');
          link.download = `${title.replace(/\s+/g, '_')}.png`;
          link.href = URL.createObjectURL(blob);
          link.click();
        });
      };
      img.src = url;
    } catch (error) {
      console.error('Failed to export PNG:', error);
    }
  };

  const exportAsSVG = () => {
    if (!chartContainerRef.current) return;

    try {
      const svgElement = chartContainerRef.current.querySelector('svg');
      if (!svgElement) return;

      const svgString = new XMLSerializer().serializeToString(svgElement);
      const blob = new Blob([svgString], { type: 'image/svg+xml;charset=utf-8' });
      const link = document.createElement('a');
      link.download = `${title.replace(/\s+/g, '_')}.svg`;
      link.href = URL.createObjectURL(blob);
      link.click();
    } catch (error) {
      console.error('Failed to export SVG:', error);
    }
  };

  return (
    <div className="my-6 p-6 bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 shadow-sm animate-fade-in-up">
      <div className="flex justify-between items-start mb-4 animate-fade-in">
        <div>
          <h3 className="text-lg font-semibold text-gray-900 dark:text-white mb-2">{title}</h3>
          {description && <p className="text-sm text-gray-600 dark:text-gray-400">{description}</p>}
        </div>
        <div className="flex items-center gap-2">
          {selectedDataset && (
            <button
              onClick={() => setSelectedDataset(null)}
              className="px-3 py-1 text-xs bg-blue-100 dark:bg-blue-900 text-blue-700 dark:text-blue-300 rounded-full hover:bg-blue-200 dark:hover:bg-blue-800 transition-all duration-200 hover:scale-105"
            >
              Show All Data
            </button>
          )}
          <div className="flex gap-1">
            <button
              onClick={exportAsPNG}
              className="px-3 py-1 text-xs bg-gradient-to-r from-shodh-red to-shodh-orange text-white rounded hover:shadow-lg transition-all duration-200 hover:scale-105 flex items-center gap-1"
              title="Export as PNG"
            >
              📸 PNG
            </button>
            <button
              onClick={exportAsSVG}
              className="px-3 py-1 text-xs bg-gray-100 dark:bg-gray-700 text-gray-700 dark:text-gray-300 rounded hover:bg-gray-200 dark:hover:bg-gray-600 transition-all duration-200 hover:scale-105 flex items-center gap-1"
              title="Export as SVG"
            >
              🎨 SVG
            </button>
          </div>
        </div>
      </div>

      <div ref={chartContainerRef} style={{ width: '100%', height: 350, position: 'relative' }}>
        <ResponsiveContainer width="100%" height="100%">
        {chartType === 'bar' && (
          <BarChart data={filteredData}>
            <CartesianGrid strokeDasharray="3 3" stroke="#374151" opacity={0.1} />
            <XAxis dataKey="name" stroke="#9CA3AF" />
            <YAxis stroke="#9CA3AF" />
            <Tooltip
              contentStyle={{
                backgroundColor: '#1F2937',
                border: '1px solid #374151',
                borderRadius: '0.5rem',
                color: '#F3F4F6'
              }}
            />
            <Legend
              onClick={(e) => handleLegendClick(e.dataKey as string)}
              wrapperStyle={{ cursor: 'pointer' }}
            />
            {data.datasets
              .filter(dataset => !selectedDataset || dataset.label === selectedDataset)
              .map((dataset, idx) => (
                <Bar
                  key={idx}
                  dataKey={dataset.label}
                  fill={COLORS[idx % COLORS.length]}
                  opacity={selectedDataset && dataset.label !== selectedDataset ? 0.3 : 1}
                />
              ))}
          </BarChart>
        )}

        {chartType === 'line' && (
          <LineChart data={chartData}>
            <CartesianGrid strokeDasharray="3 3" stroke="#374151" opacity={0.1} />
            <XAxis dataKey="name" stroke="#9CA3AF" />
            <YAxis stroke="#9CA3AF" />
            <Tooltip contentStyle={{ backgroundColor: '#1F2937', border: '1px solid #374151', borderRadius: '0.5rem' }} />
            <Legend />
            {data.datasets.map((dataset, idx) => (
              <Line key={idx} type="monotone" dataKey={dataset.label} stroke={COLORS[idx % COLORS.length]} strokeWidth={2} />
            ))}
          </LineChart>
        )}

        {chartType === 'pie' && data.datasets[0] && (
          <PieChart>
            <Pie
              data={data.labels.map((label, idx) => ({
                name: label,
                value: data.datasets[0].data[idx]
              }))}
              cx="50%"
              cy="50%"
              labelLine={false}
              label={({ name, percent }: any) => `${name}: ${(Number(percent) * 100).toFixed(0)}%`}
              outerRadius={100}
              fill="#8884d8"
              dataKey="value"
            >
              {data.labels.map((_, idx) => (
                <Cell key={`cell-${idx}`} fill={COLORS[idx % COLORS.length]} />
              ))}
            </Pie>
            <Tooltip />
          </PieChart>
        )}

        {chartType === 'doughnut' && data.datasets[0] && (
          <PieChart>
            <Pie
              data={data.labels.map((label, idx) => ({
                name: label,
                value: data.datasets[0].data[idx]
              }))}
              cx="50%"
              cy="50%"
              labelLine={false}
              label={({ name, percent }: any) => `${name}: ${(Number(percent) * 100).toFixed(0)}%`}
              innerRadius={60}
              outerRadius={100}
              fill="#8884d8"
              dataKey="value"
            >
              {data.labels.map((_, idx) => (
                <Cell key={`cell-${idx}`} fill={COLORS[idx % COLORS.length]} />
              ))}
            </Pie>
            <Tooltip />
          </PieChart>
        )}

        {chartType === 'area' && (
          <LineChart data={chartData}>
            <CartesianGrid strokeDasharray="3 3" stroke="#374151" opacity={0.1} />
            <XAxis dataKey="name" stroke="#9CA3AF" />
            <YAxis stroke="#9CA3AF" />
            <Tooltip contentStyle={{ backgroundColor: '#1F2937', border: '1px solid #374151', borderRadius: '0.5rem' }} />
            <Legend />
            {data.datasets.map((dataset, idx) => (
              <Line
                key={idx}
                type="monotone"
                dataKey={dataset.label}
                stroke={COLORS[idx % COLORS.length]}
                fill={COLORS[idx % COLORS.length]}
                fillOpacity={0.3}
                strokeWidth={2}
              />
            ))}
          </LineChart>
        )}

        {chartType === 'scatter' && (
          <ScatterChart>
            <CartesianGrid strokeDasharray="3 3" stroke="#374151" opacity={0.1} />
            <XAxis type="number" dataKey="x" name="X" stroke="#9CA3AF" />
            <YAxis type="number" dataKey="y" name="Y" stroke="#9CA3AF" />
            <ZAxis range={[60, 400]} />
            <Tooltip cursor={{ strokeDasharray: '3 3' }} contentStyle={{ backgroundColor: '#1F2937', border: '1px solid #374151', borderRadius: '0.5rem' }} />
            <Legend />
            {data.datasets.map((dataset, idx) => (
              <Scatter
                key={idx}
                name={dataset.label}
                data={dataset.data.map((val, i) => ({ x: i, y: val }))}
                fill={COLORS[idx % COLORS.length]}
              />
            ))}
          </ScatterChart>
        )}

        {chartType === 'radar' && (
          <RadarChart data={chartData}>
            <PolarGrid stroke="#374151" />
            <PolarAngleAxis dataKey="name" stroke="#9CA3AF" />
            <PolarRadiusAxis stroke="#9CA3AF" />
            <Tooltip
              contentStyle={{
                backgroundColor: '#1F2937',
                border: '1px solid #374151',
                borderRadius: '0.5rem',
                color: '#F3F4F6'
              }}
            />
            <Legend
              onClick={(e) => handleLegendClick(e.dataKey as string)}
              wrapperStyle={{ cursor: 'pointer' }}
            />
            {data.datasets
              .filter(dataset => !selectedDataset || dataset.label === selectedDataset)
              .map((dataset, idx) => (
                <Radar
                  key={idx}
                  name={dataset.label}
                  dataKey={dataset.label}
                  stroke={COLORS[idx % COLORS.length]}
                  fill={COLORS[idx % COLORS.length]}
                  fillOpacity={0.6}
                />
              ))}
          </RadarChart>
        )}

        {chartType === 'bubble' && (
          <ScatterChart>
            <CartesianGrid strokeDasharray="3 3" stroke="#374151" opacity={0.1} />
            <XAxis type="number" dataKey="x" name="X" stroke="#9CA3AF" />
            <YAxis type="number" dataKey="y" name="Y" stroke="#9CA3AF" />
            <ZAxis type="number" dataKey="z" range={[100, 1000]} name="Size" />
            <Tooltip
              cursor={{ strokeDasharray: '3 3' }}
              contentStyle={{
                backgroundColor: '#1F2937',
                border: '1px solid #374151',
                borderRadius: '0.5rem',
                color: '#F3F4F6'
              }}
            />
            <Legend />
            {data.datasets.map((dataset, idx) => (
              <Scatter
                key={idx}
                name={dataset.label}
                data={dataset.data.map((val, i) => ({
                  x: i,
                  y: Number(val),
                  z: Number(val) * 10 // Bubble size based on value
                }))}
                fill={COLORS[idx % COLORS.length]}
              />
            ))}
          </ScatterChart>
        )}
      </ResponsiveContainer>
      </div>

      {/* AI Insights Section */}
      <div className="mt-6 pt-6 border-t border-gray-200 dark:border-gray-700 animate-fade-in" style={{ animationDelay: '0.3s', animationFillMode: 'backwards' }}>
        <div className="flex items-center justify-between mb-3">
          <h4 className="text-md font-semibold text-gray-900 dark:text-white flex items-center gap-2">
            <span className="text-xl">🤖</span>
            AI Insights
          </h4>
          {!loadingInsights && insights && (
            <button
              onClick={generateInsights}
              className="px-3 py-1 text-xs bg-gray-100 dark:bg-gray-700 text-gray-700 dark:text-gray-300 rounded hover:bg-gray-200 dark:hover:bg-gray-600 transition-all duration-200 hover:scale-105"
            >
              🔄 Refresh
            </button>
          )}
        </div>

        {loadingInsights ? (
          <div className="flex items-center gap-2 text-sm text-gray-600 dark:text-gray-400">
            <div className="animate-spin rounded-full h-4 w-4 border-b-2 border-blue-500"></div>
            Analyzing data...
          </div>
        ) : insights ? (
          <div className="prose prose-sm dark:prose-invert max-w-none animate-scale-in">
            <div className="text-sm text-gray-700 dark:text-gray-300 whitespace-pre-wrap">
              {insights}
            </div>
          </div>
        ) : (
          <p className="text-sm text-gray-500 dark:text-gray-400">
            No insights available. Click refresh to generate.
          </p>
        )}
      </div>
    </div>
  );
}

function FormOutput({ title, description, fields }: {
  title: string;
  description?: string;
  fields: FormField[];
}) {
  return (
    <div className="my-6 p-6 bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700">
      <h3 className="text-lg font-semibold text-gray-900 dark:text-white mb-2">{title}</h3>
      {description && <p className="text-sm text-gray-600 dark:text-gray-400 mb-4">{description}</p>}

      <form className="space-y-4">
        {fields.map((field) => (
          <div key={field.id}>
            <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">
              {field.label} {field.required && <span className="text-red-500">*</span>}
            </label>

            {field.type === 'textarea' ? (
              <textarea
                className="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-md focus:ring-2 focus:ring-shodh-red focus:border-transparent bg-white dark:bg-gray-700 text-gray-900 dark:text-white"
                placeholder={field.placeholder}
                defaultValue={field.defaultValue}
              />
            ) : field.type === 'select' && field.options ? (
              <select className="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-md focus:ring-2 focus:ring-shodh-red focus:border-transparent bg-white dark:bg-gray-700 text-gray-900 dark:text-white">
                <option value="">Select...</option>
                {field.options.map((option) => (
                  <option key={option} value={option}>{option}</option>
                ))}
              </select>
            ) : (
              <input
                type={field.type}
                className="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-md focus:ring-2 focus:ring-shodh-red focus:border-transparent bg-white dark:bg-gray-700 text-gray-900 dark:text-white"
                placeholder={field.placeholder}
                defaultValue={field.defaultValue}
              />
            )}
          </div>
        ))}

        <button
          type="submit"
          className="px-4 py-2 bg-gradient-to-r from-shodh-red to-shodh-orange text-white rounded-md hover:shadow-lg transition-all"
        >
          Submit
        </button>
      </form>
    </div>
  );
}

function DiagramOutput({ diagramType, title, mermaid: mermaidCode, description }: { diagramType?: string; title: string; mermaid: string; description?: string }) {
  const mermaidRef = useRef<HTMLDivElement>(null);
  const [error, setError] = React.useState<string | null>(null);

  useEffect(() => {
    let mounted = true;

    const renderDiagram = async () => {
      if (!mermaidRef.current) return;

      try {
        // Initialize Mermaid with colorful theme and readable sizing
        mermaid.initialize({
          startOnLoad: false,
          theme: 'base',
          themeVariables: {
            primaryColor: '#FF6B6B',
            primaryTextColor: '#fff',
            primaryBorderColor: '#FF4757',
            lineColor: '#4ECDC4',
            secondaryColor: '#45B7D1',
            tertiaryColor: '#FFA07A',
            background: '#ffffff',
            mainBkg: '#FF6B6B',
            secondBkg: '#4ECDC4',
            tertiaryBkg: '#FFA07A',
            nodeBorder: '#2C3E50',
            clusterBkg: '#F7F9FC',
            clusterBorder: '#4ECDC4',
            titleColor: '#2C3E50',
            edgeLabelBackground: '#ffffff',
            fontSize: '16px',
          },
          securityLevel: 'loose',
          fontFamily: 'ui-sans-serif, system-ui, sans-serif',
          flowchart: {
            htmlLabels: true,
            curve: 'basis',
            padding: 20,
            nodeSpacing: 80,
            rankSpacing: 80,
          },
        });

        // Clear previous content
        mermaidRef.current.innerHTML = '';

        // Generate unique ID
        const id = `mermaid-${Math.random().toString(36).substr(2, 9)}`;

        // Render the diagram using mermaid.render
        const { svg } = await mermaid.render(id, mermaidCode);

        if (mounted && mermaidRef.current) {
          mermaidRef.current.innerHTML = svg;
          setError(null);
        }
      } catch (err: any) {
        console.error('Mermaid rendering error:', err);
        if (mounted) {
          setError(err.message || 'Failed to render diagram');
        }
      }
    };

    renderDiagram();

    return () => {
      mounted = false;
    };
  }, [mermaidCode]);

  // Get emoji based on diagram type
  const getEmoji = () => {
    switch (diagramType) {
      case 'flowchart': return '🔄';
      case 'sequence': return '📡';
      case 'class': return '🏗️';
      case 'er': return '🗄️';
      case 'state': return '🎯';
      case 'gantt': return '📅';
      case 'git': return '🌿';
      case 'journey': return '🗺️';
      default: return '📊';
    }
  };

  return (
    <div className="diagram-output bg-gradient-to-br from-purple-50 to-blue-50 dark:from-gray-800 dark:to-gray-900 rounded-lg p-4 shadow-md my-3 border border-purple-200 dark:border-purple-800 animate-fade-in-up">
      <h3 className="text-lg font-bold mb-2 text-purple-900 dark:text-purple-200 animate-fade-in">{getEmoji()} {title}</h3>
      {description && <p className="text-xs text-gray-600 dark:text-gray-400 mb-2 animate-fade-in">{description}</p>}

      {error ? (
        <div className="bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-700 rounded p-3 animate-scale-in">
          <p className="text-sm font-medium text-red-800 dark:text-red-200 mb-2">Error rendering diagram</p>
          <pre className="text-xs text-red-700 dark:text-red-300 overflow-x-auto whitespace-pre-wrap">{error}</pre>
        </div>
      ) : (
        <div
          ref={mermaidRef}
          className="mermaid-container bg-white dark:bg-gray-800 rounded p-6 overflow-x-auto animate-scale-in"
          style={{ minHeight: '300px' }}
        />
      )}
    </div>
  );
}

function SystemActionOutput({ action }: { action?: SystemAction }) {
  const [isExecuting, setIsExecuting] = React.useState(false);
  const [result, setResult] = React.useState<string | null>(null);
  const [error, setError] = React.useState<string | null>(null);

  if (!action) return null;

  const handleApprove = async () => {
    setIsExecuting(true);
    setError(null);
    setResult(null);

    try {
      const { invoke } = await import('@tauri-apps/api/core');

      // Execute the system action based on type
      if (action.Command) {
        const output = await invoke('execute_system_command', {
          action: action.Command
        });
        setResult(typeof output === 'string' ? output : JSON.stringify(output, null, 2));
      } else if (action.FileSystem) {
        const output = await invoke('execute_file_operation', {
          action: action.FileSystem
        });
        setResult(typeof output === 'string' ? output : JSON.stringify(output, null, 2));
      } else {
        setError('Unknown action type');
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setIsExecuting(false);
    }
  };

  const handleDeny = () => {
    setResult('Action denied by user');
  };

  return (
    <div className="my-4 p-4 bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-700 rounded-lg">
      <div className="flex items-start">
        <div className="flex-shrink-0">
          <svg className="h-5 w-5 text-yellow-400" viewBox="0 0 20 20" fill="currentColor">
            <path fillRule="evenodd" d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z" clipRule="evenodd" />
          </svg>
        </div>
        <div className="ml-3 flex-1">
          <h3 className="text-sm font-medium text-yellow-800 dark:text-yellow-200">
            System Action Requested
          </h3>
          <div className="mt-2 text-sm text-yellow-700 dark:text-yellow-300">
            <p>This action requires your approval before execution.</p>
            <pre className="mt-2 p-2 bg-yellow-100 dark:bg-yellow-900/40 rounded text-xs overflow-x-auto">
              {JSON.stringify(action, null, 2)}
            </pre>
          </div>

          {!result && !error && (
            <div className="mt-4 flex gap-2">
              <button
                onClick={handleApprove}
                disabled={isExecuting}
                className="px-3 py-1 bg-green-600 text-white rounded text-sm hover:bg-green-700 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {isExecuting ? 'Executing...' : 'Approve & Execute'}
              </button>
              <button
                onClick={handleDeny}
                disabled={isExecuting}
                className="px-3 py-1 bg-red-600 text-white rounded text-sm hover:bg-red-700 disabled:opacity-50"
              >
                Deny
              </button>
            </div>
          )}

          {result && (
            <div className="mt-4 p-3 bg-green-50 dark:bg-green-900/20 border border-green-200 dark:border-green-700 rounded">
              <p className="text-sm font-medium text-green-800 dark:text-green-200 mb-2">✓ Action Executed Successfully</p>
              <pre className="text-xs text-green-700 dark:text-green-300 overflow-x-auto whitespace-pre-wrap">
                {result}
              </pre>
            </div>
          )}

          {error && (
            <div className="mt-4 p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-700 rounded">
              <p className="text-sm font-medium text-red-800 dark:text-red-200 mb-2">✗ Execution Failed</p>
              <pre className="text-xs text-red-700 dark:text-red-300 overflow-x-auto whitespace-pre-wrap">
                {error}
              </pre>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
