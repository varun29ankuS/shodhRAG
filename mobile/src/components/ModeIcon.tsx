/**
 * A conversation's kind as an icon in its own colour: Research a book in
 * Shodh's accent, Code `</>` in blue, Creator a sparkle in violet. Named for
 * screen readers, unless `decorative` (next to a label that already says it).
 */
import { BookOpen, CodeXml, Sparkles } from 'lucide-react';
import { KIND_LABELS, type ConversationKind } from '../lib/modes';

export function ModeIcon({ kind, decorative = false }: { kind: ConversationKind; decorative?: boolean }) {
  const Icon = kind === 'code' ? CodeXml : kind === 'creator' ? Sparkles : BookOpen;
  const named = decorative ? { 'aria-hidden': true as const } : { role: 'img', 'aria-label': KIND_LABELS[kind] };
  return (
    <span className={`mode-icon mode-${kind}`} data-kind={kind} {...named}>
      <Icon aria-hidden="true" className="icon-small" />
    </span>
  );
}
