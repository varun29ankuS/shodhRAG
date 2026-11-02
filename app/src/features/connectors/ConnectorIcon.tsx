import React from 'react';
import { SlackLogo } from './SlackLogo';
import { TelegramLogo } from './TelegramLogo';
import { GitHubLogo } from './GitHubLogo';

/** A connector's logo, small, beside its name (decorative). */
export function ConnectorIcon({ connector, className }: { connector: string; className?: string }) {
  if (connector === 'telegram') return <TelegramLogo className={className} />;
  if (connector === 'slack') return <SlackLogo className={className} />;
  if (connector === 'github') return <GitHubLogo className={className} />;
  return null;
}
