/**
 * The interactive answer blocks (sketches, plots, simulations, figures,
 * structured diagrams), loaded with the first answer that has one instead
 * of at startup. Each shows an empty frame while its code loads.
 */
import React, { Suspense, lazy } from 'react';

type BlockProps = { source: string };

const SvgBlockView = lazy(() => import('./SvgSketch').then(m => ({ default: m.SvgBlock })));
const PlotBlockView = lazy(() => import('./PlotView').then(m => ({ default: m.PlotBlock })));
const SimulationBlockView = lazy(() => import('./SimulationView').then(m => ({ default: m.SimulationBlock })));
const FigureBlockView = lazy(() => import('./FigureBlock').then(m => ({ default: m.FigureBlock })));
const DiagramBlockView = lazy(() => import('./DiagramBlock').then(m => ({ default: m.DiagramBlock })));

function BlockLoading() {
  return <div className="my-4 min-h-[160px] rounded-xl border border-shodh-border bg-shodh-surface" role="status" aria-busy="true" aria-label="Loading" />;
}

function lazyBlock(View: React.ComponentType<BlockProps>) {
  return function LazyBlock({ source }: BlockProps) {
    return (
      <Suspense fallback={<BlockLoading />}>
        <View source={source} />
      </Suspense>
    );
  };
}

export const SvgBlock = lazyBlock(SvgBlockView);
export const PlotBlock = lazyBlock(PlotBlockView);
export const SimulationBlock = lazyBlock(SimulationBlockView);
export const FigureBlock = lazyBlock(FigureBlockView);
export const DiagramBlock = lazyBlock(DiagramBlockView);
