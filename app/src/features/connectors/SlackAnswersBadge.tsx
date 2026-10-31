import React, { useEffect, useState } from 'react';
import { onSlackStatus, slackApi } from './api';
import type { SlackStatus } from './slackModel';
import { SlackLogo } from './SlackLogo';

/** The Shodh workspace Slack answers from, and the Slack workspace that asks. */
export interface SlackAnswerSource {
  workspaceId: string;
  team: string;
}

function sourceOf(status: SlackStatus): SlackAnswerSource | null {
  const ws = status.answerWorkspace;
  if (!ws || !ws.available || !status.installation || status.state === 'not_set_up') return null;
  return { workspaceId: ws.id, team: status.installation.teamName };
}

/** Which workspace Slack answers from, kept current with the Slack card. */
export function useSlackAnswerSource(): SlackAnswerSource | null {
  const [source, setSource] = useState<SlackAnswerSource | null>(null);
  useEffect(() => {
    let cancelled = false;
    slackApi
      .status()
      .then(status => { if (!cancelled) setSource(sourceOf(status)); })
      .catch(err => console.error('The Slack connector status could not be read:', err));
    const unsubscribe = onSlackStatus(status => setSource(sourceOf(status)));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);
  return source;
}

/** "Answers in Slack" on the workspace Slack answers from: what is added to it can be read there. */
export function SlackAnswersBadge({ source, workspaceId }: { source: SlackAnswerSource | null; workspaceId: string }) {
  if (!source || source.workspaceId !== workspaceId) return null;
  const title = `People in ${source.team} get answers from this workspace in Slack. What is added here can be read there.`;
  return (
    <span
      title={title}
      aria-label={title}
      className="inline-flex items-center gap-1 shrink-0 text-caption font-medium px-1.5 py-0.5 rounded-full bg-shodh-raised text-shodh-warning"
    >
      <SlackLogo className="w-3 h-3" />
      Answers in Slack
    </span>
  );
}
