import React from 'react';
import { Citation, CitationBadge } from '../components/CitationBadge';

export interface SearchResult {
  id: string;
  score: number;
  snippet: string;
  citation: {
    title: string;
    authors: string[];
    source: string;
    year: string;
    url?: string;
    doi?: string;
    page_numbers?: string;
  };
  metadata: Record<string, string>;
  sourceFile: string;
  pageNumber?: number;
  lineRange?: [number, number];
  surroundingContext: string;
}

/**
 * Parse AI response text and inject inline citation badges
 * Looks for patterns like [1], [2,3], etc. and replaces them with CitationBadge components
 */
export function parseResponseWithCitations(
  text: string,
  searchResults?: SearchResult[],
  onFollowUpQuery?: (query: string) => void,
  onOpenUrl?: (url: string) => void,
  onViewInArtifact?: (citation: any) => void
): React.ReactNode[] {
  if (!searchResults || searchResults.length === 0) {
    return [text];
  }

  // Clean up standalone citation lines that are ONLY citations
  const citationCleanupPattern = '\\[(?:Document\\s+\\d+(?:\\s*,\\s*Document\\s+\\d+)*|\\d+(?:\\s*,\\s*\\d+)*)\\]';

  // Split text into lines to process each line separately
  const lines = text.split('\n');
  const processedLines = lines.map(line => {
    // Skip empty lines
    if (!line.trim()) {
      return line;
    }

    // Remove lines that are ONLY citations (standalone citation lines)
    if (new RegExp(`^\\s*${citationCleanupPattern}\\s*$`, 'i').test(line)) {
      return '';
    }

    // Keep citations inline - don't move them
    return line;
  });

  text = processedLines.join('\n');

  // Clean up multiple consecutive blank lines (max 2 newlines in a row)
  text = text.replace(/\n{3,}/g, '\n\n');

  // Create citation map from search results (1-indexed to match LLM output)
  const citationMap: Map<number, Citation> = new Map();

  searchResults.forEach((result, index) => {
    const citationNumber = index + 1; // 1-indexed (matches backend numbering)

    citationMap.set(citationNumber, {
      id: result.id,
      sourceFile: result.sourceFile,
      pageNumber: result.pageNumber,
      lineRange: result.lineRange as [number, number] | undefined,
      snippet: result.snippet,
      surroundingContext: result.surroundingContext,
      citationTitle: result.citation?.title || result.sourceFile?.split(/[/\\]/).pop() || 'Source',
    });
  });

  // Normalize alternative citation formats before parsing:
  // 【5†L1-L3】 → [5]  (ChatGPT-style citations some LLMs generate)
  // 【5†source】 → [5]
  text = text.replace(/【(\d+)†[^】]*】/g, '[$1]');

  // Parse text for citation markers like [1], [2], [1,2,3], [Document 1], [Document 1, Document 2], etc.
  const citationPattern = /\[(?:Document\s+\d+(?:\s*,\s*Document\s+\d+)*|\d+(?:\s*,\s*\d+)*)\]/gi;
  const parts: React.ReactNode[] = [];
  let lastIndex = 0;
  let match;

  while ((match = citationPattern.exec(text)) !== null) {
    // Add text before the citation
    if (match.index > lastIndex) {
      parts.push(text.substring(lastIndex, match.index));
    }

    // Extract all numbers from the match (handles both "[1,2]" and "[Document 1, Document 2]")
    const numbersMatch = match[0].match(/\d+/g);
    const citationNumbers = numbersMatch
      ? numbersMatch.map((n) => parseInt(n)).filter((n) => !isNaN(n))
      : [];

    // Add citation badge(s)
    // First check if we have valid citations for these numbers
    const validCitations = citationNumbers.filter(num => citationMap.has(num));

    if (validCitations.length > 0) {
      // Render badges for valid citations only
      validCitations.forEach((num, idx) => {
        const citation = citationMap.get(num);
        parts.push(
          <CitationBadge
            key={`${match.index}-${num}`}
            citation={citation!}
            index={num}
            onFollowUpQuery={onFollowUpQuery}
            onOpenUrl={onOpenUrl}
            onViewInArtifact={onViewInArtifact}
          />
        );
        // Add space separator if not last citation
        if (idx < validCitations.length - 1) {
          parts.push(' ');
        }
      });
    } else {
      // No valid citations found, keep original text
      parts.push(match[0]);
    }

    lastIndex = match.index + match[0].length;
  }

  // Add remaining text
  if (lastIndex < text.length) {
    parts.push(text.substring(lastIndex));
  }

  return parts.length > 0 ? parts : [text];
}

/**
 * Extract citation markers from text to determine which citations are referenced
 */
export function extractCitationNumbers(text: string): number[] {
  const citationPattern = /\[(?:Document\s+)?(\d+(?:,\s*\d+)*)\]/gi;
  const numbers = new Set<number>();
  let match;

  while ((match = citationPattern.exec(text)) !== null) {
    match[1]
      .split(',')
      .map((n) => parseInt(n.trim()))
      .filter((n) => !isNaN(n))
      .forEach((n) => numbers.add(n));
  }

  return Array.from(numbers).sort((a, b) => a - b);
}

/**
 * Format citations list for display at end of response
 */
export function formatCitationsList(searchResults: SearchResult[]): string {
  if (searchResults.length === 0) return '';

  const citations = searchResults.map((result, index) => {
    const fileName = result.sourceFile.split(/[/\\]/).pop();
    const location = result.lineRange
      ? `lines ${result.lineRange[0]}-${result.lineRange[1]}`
      : result.pageNumber
      ? `page ${result.pageNumber}`
      : '';

    const title = result.citation?.title || fileName || 'Untitled';
    return `${index + 1}. ${title} - ${fileName}${
      location ? ` (${location})` : ''
    }`;
  });

  return `\n\n### Sources\n${citations.join('\n')}`;
}
