import React from 'react';
import { Hammer } from 'lucide-react';
import { cn } from '../../lib/utils';
import { tr } from '../../i18n/tr';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

/**
 * Under an Ask answer to a request to build a page or an app (`asksToBuild`): Ask cannot
 * build one, Creator can. "Build in Creator" opens a new Creator chat with the request in
 * its message box, the user's own words, for them to send. Where Code is off in
 * Settings (Creator works in a folder of its own), the card says so and opens Settings.
 */
export function BuildInCreatorCard({
  codeAvailable,
  onBuild,
  onOpenSettings,
}: {
  codeAvailable: boolean;
  onBuild: () => void;
  onOpenSettings: () => void;
}) {
  return (
    <section
      aria-label={tr('ask.buildInCreatorCard.buildInCreator')}
      data-testid="build-in-creator"
      className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3"
    >
      <Hammer className="w-4 h-4 shrink-0 text-shodh-accent-text" aria-hidden="true" />
      <p className="m-0 flex-1 min-w-[12rem] text-small text-shodh-text-secondary">
        {codeAvailable ? tr('ask.buildInCreatorCard.askCantBuild') : tr('ask.buildInCreatorCard.creatorNeedsCode')}
      </p>
      <button
        type="button"
        onClick={codeAvailable ? onBuild : onOpenSettings}
        className={cn(
          'inline-flex items-center gap-2 h-8 px-3 rounded-lg bg-shodh-accent text-shodh-on-accent text-small font-semibold hover:bg-shodh-accent-hover transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <Hammer className="w-3.5 h-3.5" aria-hidden="true" />
        {codeAvailable ? tr('ask.buildInCreatorCard.buildInCreator') : tr('ask.buildInCreatorCard.openSettings')}
      </button>
    </section>
  );
}

export default BuildInCreatorCard;
