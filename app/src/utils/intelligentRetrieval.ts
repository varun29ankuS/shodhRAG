import { invoke } from '@tauri-apps/api/core';

interface SearchRequest {
  query: string;
  max_results: number;
  space_id?: string | null;
  filters?: {
    spaceIds?: string[];
  };
}

interface Citation {
  title: string;
  authors: string[];
  source: string;
  year: string;
  url?: string;
  doi?: string;
  pageNumbers?: string;
}

interface SearchResult {
  id: string;
  score: number;
  snippet: string;
  citation: Citation;
  metadata: Record<string, any>;
  sourceFile: string;
  pageNumber?: number;
  lineRange?: [number, number];
  surroundingContext: string;
}

interface DecisionMetadata {
  intent: string;
  shouldRetrieve: boolean;
  strategy: string;
  reasoning: string;
  confidence: number;
}

interface IntelligentSearchResult {
  results: SearchResult[];
  decision: DecisionMetadata;
  rewrittenQuery?: string;
}

interface RewrittenQuery {
  originalQuery: string;
  rewrittenQuery: string;
  explanation: string;
  usedContext: boolean;
  shouldRetrieve: boolean;
  retrievalReason: string;
}

interface SearchWithRewritingResult {
  queryRewriting: RewrittenQuery;
  results: SearchResult[];
  totalResults: number;
}

export async function intelligentSearch(
  query: string,
  spaceId: string | null,
  maxResults: number = 5
): Promise<IntelligentSearchResult> {
  console.log('intelligentSearch called:', { query, spaceId, maxResults });

  // Try query rewriting first for better retrieval quality
  try {
    const rewriteResult = await invoke<SearchWithRewritingResult>('search_with_query_rewriting', {
      query,
      spaceId,
      maxResults,
    });

    const rewritten = rewriteResult.queryRewriting;
    const wasRewritten = rewritten.rewrittenQuery !== rewritten.originalQuery;

    console.log(`Query rewriting: "${query}" → "${rewritten.rewrittenQuery}" (rewritten: ${wasRewritten})`);
    console.log(`Retrieved ${rewriteResult.results.length} documents via rewriting path`);

    return {
      results: rewriteResult.results,
      decision: {
        intent: 'search',
        shouldRetrieve: rewritten.shouldRetrieve,
        strategy: wasRewritten ? 'rewritten' : 'direct',
        reasoning: rewritten.explanation || rewritten.retrievalReason,
        confidence: 0.8,
      },
      rewrittenQuery: wasRewritten ? rewritten.rewrittenQuery : undefined,
    };
  } catch (rewriteError) {
    console.warn('Query rewriting unavailable, falling back to direct search:', rewriteError);
  }

  // Fallback: direct search without rewriting
  const searchRequest: SearchRequest = {
    query,
    max_results: maxResults,
    space_id: spaceId,
  };

  try {
    const response = await invoke<IntelligentSearchResult>('search_documents', { request: searchRequest });
    console.log(`Retrieved ${response.results.length} documents`);
    return response;
  } catch (error) {
    console.error('Search failed:', error);
    return {
      results: [],
      decision: {
        intent: 'error',
        shouldRetrieve: false,
        strategy: 'none',
        reasoning: `Search failed: ${error}`,
        confidence: 0
      }
    } as IntelligentSearchResult;
  }
}

export async function trackUserMessage(message: string): Promise<void> {
  try {
    await invoke('track_user_message', { message });
  } catch (error) {
    console.warn('Failed to track user message:', error);
  }
}

export async function trackAssistantMessage(message: string): Promise<void> {
  try {
    await invoke('track_assistant_message', { message });
  } catch (error) {
    console.warn('Failed to track assistant message:', error);
  }
}
