/**
 * The send options of a question from the Library's sources, for the Ask composer
 * and the conversation dock alike: when only some sources are included ("Include
 * this folder when answering"), answers search just those. Type imports only:
 * unit-tested under Node.
 */
import type { SendOptions } from './types';

export interface OptionSource {
  id: string;
  name: string;
  selected: boolean;
}

export function sendOptionsFor(sources: readonly OptionSource[]): SendOptions {
  const included = sources.filter(s => s.selected);
  const first = included[0] ?? null;
  const limited = included.length > 0 && included.length < sources.length;
  return {
    spaceId: first?.id ?? null,
    spaceName: first?.name ?? null,
    sourceIds: limited ? included.map(s => s.id) : [],
  };
}

/** Equal for equal options: lets callers keep one options object while the sources only re-render. */
export function sendOptionsKey(options: SendOptions): string {
  return JSON.stringify([options.spaceId, options.spaceName, options.sourceIds ?? []]);
}
