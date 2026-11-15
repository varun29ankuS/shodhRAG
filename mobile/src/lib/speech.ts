/**
 * An answer read aloud: its text without the markdown and without the
 * citation markers, then its sources, named from the citation metadata
 * ("Self-RAG, page 4"), never from the answer's words. A `[n]` marker only
 * says which source it is.
 */
import { answerText, citedNumbers, type AnswerState, type Passage } from './answer';
import { isPhotoPath } from './photo';

/** A source as it is said: its title (without a file extension) and its page. */
export function spokenSource(passage: Passage): string {
  if (isPhotoPath(passage.path)) return 'your photo';
  const title = passage.web ? passage.title : passage.title.replace(/\.(pdf|docx?|pptx?|xlsx?|md|txt|html?|epub|csv)$/i, '');
  return passage.page !== null && !passage.web ? `${title}, page ${passage.page}` : title;
}

/** The answer's words to say: no markdown, no citation markers. */
export function spokenText(text: string): string {
  return text
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/\[(\d+(?:\s*[,;]\s*\d+)*)\]/g, '')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/^\s{0,3}#{1,6}\s+/gm, '')
    .replace(/^\s*[-*+]\s+/gm, '')
    .replace(/^\s*\|.*\|\s*$/gm, '')
    .replace(/[*_`~>]+/g, '')
    .replace(/[ \t]+([.,;:!?])/g, '$1')
    .replace(/[ \t]{2,}/g, ' ')
    .replace(/(\n[ \t]*)+\n/g, '\n')
    .trim();
}

/** What is read aloud for `answer`. */
export function spokenAnswer(answer: AnswerState): string {
  const text = answerText(answer);
  const sources = citedNumbers(text)
    .map(n => answer.passages.find(p => p.n === n))
    .filter((p): p is Passage => p !== undefined)
    .map(spokenSource);
  const unique = [...new Set(sources)];
  const said = spokenText(text);
  if (unique.length === 0) return said;
  return `${said}\n${unique.length === 1 ? 'Source' : 'Sources'}: ${unique.join('; ')}.`;
}
