import React from 'react';
import { CONNECTORS, type ConnectorId } from './registry';
import SlackCard from './SlackCard';

/** The card of each connector; a new connector adds its card here and its entry in the registry. */
const CARDS: Record<ConnectorId, React.ComponentType<{ summary: string }>> = {
  slack: SlackCard,
};

/** Connectors: Shodh working inside other tools, one card per connector. */
export default function ConnectorsView() {
  return (
    <div className="h-full overflow-y-auto bg-shodh-ground text-shodh-text">
      <div className="max-w-[880px] mx-auto px-8 py-7 flex flex-col gap-6">
        <header className="flex flex-col gap-1.5">
          <h1 className="m-0 text-2xl font-bold">Connectors</h1>
          <p className="m-0 text-sm text-shodh-text-muted">
            Tools Shodh works in. Each connector says what it can reach, and nothing connects until you set it up.
          </p>
        </header>
        {CONNECTORS.map(connector => {
          const Card = CARDS[connector.id];
          return <Card key={connector.id} summary={connector.summary} />;
        })}
      </div>
    </div>
  );
}
