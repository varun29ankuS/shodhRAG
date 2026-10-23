/**
 * The connectors the Connectors view lists, in order. A connector adds an entry here and a
 * card component in `ConnectorsView.tsx`. Pure, so `node --test` loads it directly.
 */

export type ConnectorId = 'slack';

export interface ConnectorInfo {
  id: ConnectorId;
  name: string;
  /** What it does for the user, in one sentence. */
  summary: string;
}

export const CONNECTORS: readonly ConnectorInfo[] = [
  {
    id: 'slack',
    name: 'Slack',
    summary:
      'People in your Slack workspace mention @shodh or message it, and Shodh answers from one workspace you choose, with citations.',
  },
];
