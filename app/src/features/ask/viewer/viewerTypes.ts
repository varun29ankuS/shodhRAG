/** Outcome of locating the cited passage, shown as the viewer's notice. */
export interface LocateResult {
  status: 'searching' | 'found' | 'approximate' | 'notFound';
  message: string;
}

export const VIEWER_FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
