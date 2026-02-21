declare module 'react-force-graph-2d' {
  import { FC, MutableRefObject } from 'react';

  export interface NodeObject {
    id: string | number;
    x?: number;
    y?: number;
    vx?: number;
    vy?: number;
    fx?: number | null;
    fy?: number | null;
    [key: string]: any;
  }

  export interface LinkObject {
    source: string | number | NodeObject;
    target: string | number | NodeObject;
    [key: string]: any;
  }

  export interface GraphData {
    nodes: NodeObject[];
    links: LinkObject[];
  }

  export interface ForceGraph2DProps {
    ref?: MutableRefObject<any>;
    graphData?: GraphData;
    width?: number;
    height?: number;
    backgroundColor?: string;
    nodeId?: string | ((node: NodeObject) => string);
    nodeLabel?: string | ((node: NodeObject) => string);
    nodeColor?: string | ((node: NodeObject) => string);
    nodeAutoColorBy?: string | ((node: NodeObject) => string | null);
    nodeCanvasObject?: (node: NodeObject, ctx: CanvasRenderingContext2D, globalScale: number) => void;
    nodeCanvasObjectMode?: string | ((node: NodeObject) => string);
    nodeVal?: number | string | ((node: NodeObject) => number);
    nodeRelSize?: number;
    linkSource?: string;
    linkTarget?: string;
    linkLabel?: string | ((link: LinkObject) => string);
    linkColor?: string | ((link: LinkObject) => string);
    linkAutoColorBy?: string | ((link: LinkObject) => string | null);
    linkWidth?: number | string | ((link: LinkObject) => number);
    linkCurvature?: number | string | ((link: LinkObject) => number);
    linkCanvasObject?: (link: LinkObject, ctx: CanvasRenderingContext2D, globalScale: number) => void;
    linkCanvasObjectMode?: string | ((link: LinkObject) => string);
    linkDirectionalArrowLength?: number | string | ((link: LinkObject) => number);
    linkDirectionalArrowColor?: string | ((link: LinkObject) => string);
    linkDirectionalArrowRelPos?: number | string | ((link: LinkObject) => number);
    linkDirectionalParticles?: number | string | ((link: LinkObject) => number);
    linkDirectionalParticleSpeed?: number | string | ((link: LinkObject) => number);
    linkDirectionalParticleWidth?: number | string | ((link: LinkObject) => number);
    linkDirectionalParticleColor?: string | ((link: LinkObject) => string);
    onNodeClick?: (node: NodeObject, event: MouseEvent) => void;
    onNodeRightClick?: (node: NodeObject, event: MouseEvent) => void;
    onNodeHover?: (node: NodeObject | null, previousNode: NodeObject | null) => void;
    onNodeDrag?: (node: NodeObject, translate: { x: number; y: number }) => void;
    onNodeDragEnd?: (node: NodeObject, translate: { x: number; y: number }) => void;
    onLinkClick?: (link: LinkObject, event: MouseEvent) => void;
    onLinkRightClick?: (link: LinkObject, event: MouseEvent) => void;
    onLinkHover?: (link: LinkObject | null, previousLink: LinkObject | null) => void;
    onBackgroundClick?: (event: MouseEvent) => void;
    onBackgroundRightClick?: (event: MouseEvent) => void;
    onZoom?: (transform: { k: number; x: number; y: number }) => void;
    onZoomEnd?: (transform: { k: number; x: number; y: number }) => void;
    cooldownTicks?: number;
    cooldownTime?: number;
    warmupTicks?: number;
    d3AlphaMin?: number;
    d3AlphaDecay?: number;
    d3VelocityDecay?: number;
    dagMode?: 'td' | 'bu' | 'lr' | 'rl' | 'zout' | 'zin' | 'radialout' | 'radialin' | null;
    dagLevelDistance?: number;
    dagNodeFilter?: (node: NodeObject) => boolean;
    onDagError?: (loopNodeIds: (string | number)[]) => void;
    enableNodeDrag?: boolean;
    enableZoomInteraction?: boolean;
    enablePanInteraction?: boolean;
    enablePointerInteraction?: boolean;
    autoPauseRedraw?: boolean;
    minZoom?: number;
    maxZoom?: number;
    onEngineStop?: () => void;
    onEngineTick?: () => void;
  }

  export interface ForceGraph2DMethods {
    zoomToFit: (duration?: number, padding?: number) => void;
    zoom: (scale: number, duration?: number) => void;
    centerAt: (x?: number, y?: number, duration?: number) => void;
    emitParticle: (link: LinkObject) => void;
    d3Force: (forceName: string, force?: any) => any;
    d3ReheatSimulation: () => void;
    pauseAnimation: () => void;
    resumeAnimation: () => void;
    screen2GraphCoords: (x: number, y: number) => { x: number; y: number };
    graph2ScreenCoords: (x: number, y: number) => { x: number; y: number };
    getGraphCanvas: () => HTMLCanvasElement;
  }

  const ForceGraph2D: FC<ForceGraph2DProps>;
  export default ForceGraph2D;
}