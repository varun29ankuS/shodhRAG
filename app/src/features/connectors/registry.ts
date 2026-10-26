/**
 * The connectors the Connectors view lists, in order: one tile each in the gallery, and
 * one page. A connector adds an entry here and its tile and page in `ConnectorsView.tsx`.
 * Pure, so `node --test` loads it directly.
 */

export type ConnectorId = 'slack';

export interface ConnectorInfo {
  id: ConnectorId;
  name: string;
  /** What it does for the user, in one line (the tile). */
  summary: string;
}

export const CONNECTORS: readonly ConnectorInfo[] = [
  {
    id: 'slack',
    name: 'Slack',
    summary: 'Your team asks @shodh in Slack and gets cited answers from one workspace you choose.',
  },
];
