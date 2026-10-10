import { Suspense, lazy, useMemo } from 'react';
import type { Artifact } from './EnhancedArtifactPanel';
import { tryParseChartSpec } from '../utils/artifactExtractor';
import { ChartTitle, ChartUnparsed, chartTitleColor } from './chartFrame';

export interface ChartArtifactProps {
  artifact: Artifact;
  theme: string;
  /** Plot height in pixels (the focus view enlarges it to zoom). */
  height?: number;
}

// recharts (with its d3 and state libraries) is the largest library in the
// app; it loads with the first chart instead of at startup.
const ChartArtifactView = lazy(() => import('./ChartArtifactView').then(m => ({ default: m.ChartArtifactView })));

/** The chart's frame (title, plot area) while recharts loads, so nothing moves when it lands. */
function ChartPlaceholder({ artifact, theme, height = 320 }: ChartArtifactProps) {
  const spec = useMemo(() => tryParseChartSpec(artifact.content), [artifact.content]);
  if (!spec) return <ChartUnparsed content={artifact.content} />;
  return (
    <div aria-busy="true">
      {spec.title && <ChartTitle title={spec.title} color={chartTitleColor(theme)} />}
      <div style={{ width: '100%', height, padding: '4px 8px 8px' }} />
    </div>
  );
}

export function ChartArtifact(props: ChartArtifactProps) {
  return (
    <Suspense fallback={<ChartPlaceholder {...props} />}>
      <ChartArtifactView {...props} />
    </Suspense>
  );
}
