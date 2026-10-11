/**
 * Hooks of the snippet views: a snippet's image (stored, or drawn from the
 * PDF the first time and then stored), the vision capability, and a
 * snippet list that follows `research-changed`.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { onResearchChanged, researchApi, toResearchError } from './api';
import { renderSnippet, withPdf } from './snippetRender';
import type { Snippet, SnippetListQuery, VisionCapability } from './types';

/** Images loaded this session, by snippet id and version (few, small enough). */
const imageCache = new Map<string, string>();
const IMAGE_CACHE_ENTRIES = 80;
/** Draws in flight per snippet, so a grid and the pop-out share one. */
const inflight = new Map<string, Promise<string>>();

function cacheKey(s: Pick<Snippet, 'id' | 'statementId'>): string {
  return `${s.id}|${s.statementId}`;
}

function remember(key: string, png: string): void {
  imageCache.delete(key);
  imageCache.set(key, png);
  while (imageCache.size > IMAGE_CACHE_ENTRIES) {
    const oldest = imageCache.keys().next().value;
    if (oldest === undefined) break;
    imageCache.delete(oldest);
  }
}

/**
 * The snippet's PNG (base64): the stored image, else a pdf.js render of
 * its region, which is then stored so it is drawn only once (snippets the
 * assistant made have no image until first shown).
 */
export async function loadSnippetImage(s: Snippet): Promise<string> {
  const key = cacheKey(s);
  const cached = imageCache.get(key);
  if (cached) return cached;
  const running = inflight.get(key);
  if (running) return running;
  const task = (async () => {
    const stored = s.hasImage ? await researchApi.snippetImage(s.id) : null;
    if (stored) return stored;
    const rendered = await withPdf(s.filePath, doc => renderSnippet(doc, s.page, s.rect));
    try {
      await researchApi.setSnippetImage(s.id, rendered.png);
    } catch (error) {
      // Shown anyway; it is drawn again next time.
      console.warn('Storing the snippet image failed:', toResearchError(error).message);
    }
    return rendered.png;
  })();
  inflight.set(key, task);
  try {
    const png = await task;
    remember(key, png);
    return png;
  } finally {
    inflight.delete(key);
  }
}

export type ImageState = { status: 'loading' } | { status: 'ready'; png: string } | { status: 'error'; message: string };

export function useSnippetImage(snippet: Snippet | null): ImageState {
  const [state, setState] = useState<ImageState>({ status: 'loading' });
  const id = snippet?.id ?? null;
  const version = snippet?.statementId ?? null;
  const ref = useRef(snippet);
  ref.current = snippet;
  useEffect(() => {
    const current = ref.current;
    if (!current) return;
    let cancelled = false;
    const cached = imageCache.get(cacheKey(current));
    setState(cached ? { status: 'ready', png: cached } : { status: 'loading' });
    if (cached) return;
    loadSnippetImage(current)
      .then(png => {
        if (!cancelled) setState({ status: 'ready', png });
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'error', message: toResearchError(error).message });
      });
    return () => {
      cancelled = true;
    };
  }, [id, version]);
  return state;
}

/** Whether a vision model can transcribe equations, asked once per mount. */
export function useVisionCapability(): VisionCapability | null {
  const [capability, setCapability] = useState<VisionCapability | null>(null);
  useEffect(() => {
    let cancelled = false;
    researchApi
      .visionCapability()
      .then(c => {
        if (!cancelled) setCapability(c);
      })
      .catch(error => {
        if (!cancelled) setCapability({ available: false, model: null, reason: toResearchError(error).message });
      });
    return () => {
      cancelled = true;
    };
  }, []);
  return capability;
}

export type ListState = { status: 'loading' } | { status: 'ready'; items: Snippet[] } | { status: 'error'; message: string };

/** Snippets matching `query`, reloaded after any snippet change. */
export function useSnippetList(query: SnippetListQuery, enabled = true): { state: ListState; reload: () => void; replace: (id: string, next: Snippet | null) => void } {
  const [state, setState] = useState<ListState>({ status: 'loading' });
  const [tick, setTick] = useState(0);
  const key = JSON.stringify(query);
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    setState(s => (s.status === 'ready' ? s : { status: 'loading' }));
    researchApi
      .listSnippets(JSON.parse(key) as SnippetListQuery)
      .then(items => {
        if (!cancelled) setState({ status: 'ready', items });
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'error', message: toResearchError(error).message });
      });
    return () => {
      cancelled = true;
    };
  }, [key, tick, enabled]);
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    onResearchChanged(change => {
      if (change.kind === 'result') return;
      setTick(t => t + 1);
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  const reload = useCallback(() => setTick(t => t + 1), []);
  const replace = useCallback((id: string, next: Snippet | null) => {
    setState(s => {
      if (s.status !== 'ready') return s;
      const items = next ? s.items.map(i => (i.id === id ? next : i)) : s.items.filter(i => i.id !== id);
      return { status: 'ready', items };
    });
  }, []);
  return { state, reload, replace };
}
