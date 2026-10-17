import type { RevisionReason } from './events';

/**
 * What a follow-up turn is doing, in words: present tense while the run is
 * live, past tense once it has finished (the row then records what happened).
 */
export function revisionText(
  reason: RevisionReason,
  flagged: number,
  missingNeeds: readonly string[],
  live: boolean,
): string {
  const statements = `${flagged} flagged ${flagged === 1 ? 'statement' : 'statements'}`;
  const fix = `${live ? 'Re-checking' : 'Re-checked'} ${statements} against the sources`;
  const searchVerb = live ? 'Searching' : 'Searched';
  const search = missingNeeds.length === 1
    ? `${searchVerb} for a part no passage covered: ${missingNeeds[0]}`
    : `${searchVerb} for ${missingNeeds.length} parts no passage covered`;
  if (reason === 'retrieve') {
    return `${live ? 'Searching' : 'Searched'} your documents for the question`;
  }
  if (reason === 'repair') return fix;
  if (reason === 'coverage') return search;
  return `${fix}; ${search.charAt(0).toLowerCase()}${search.slice(1)}`;
}
