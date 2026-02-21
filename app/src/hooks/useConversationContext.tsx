import { useState, useEffect, useCallback } from 'react';

export interface ConversationContext {
  filesDiscussed: Set<string>;
  functionsDiscussed: Set<string>;
  topics: string[];
  lastQuery: string;
  workspace?: string;
  sessionStart: number;
}

interface CodeReference {
  filePath: string;
  lineNumber?: number;
  function?: string;
  snippet?: string;
}

interface UseConversationContextOptions {
  workspace?: string;
  persistKey?: string;
}

export function useConversationContext({
  workspace,
  persistKey = 'conversation_context'
}: UseConversationContextOptions = {}) {
  const [context, setContext] = useState<ConversationContext>(() => {
    // Try to load from localStorage
    const stored = localStorage.getItem(persistKey);
    if (stored) {
      try {
        const parsed = JSON.parse(stored);
        return {
          ...parsed,
          filesDiscussed: new Set(parsed.filesDiscussed || []),
          functionsDiscussed: new Set(parsed.functionsDiscussed || []),
          topics: parsed.topics || [],
          lastQuery: parsed.lastQuery || '',
          workspace: workspace || parsed.workspace,
          sessionStart: parsed.sessionStart || Date.now()
        };
      } catch (e) {
        console.error('Failed to parse stored context:', e);
      }
    }

    // Default context
    return {
      filesDiscussed: new Set<string>(),
      functionsDiscussed: new Set<string>(),
      topics: [],
      lastQuery: '',
      workspace,
      sessionStart: Date.now()
    };
  });

  // Persist to localStorage whenever context changes
  useEffect(() => {
    const toStore = {
      filesDiscussed: Array.from(context.filesDiscussed),
      functionsDiscussed: Array.from(context.functionsDiscussed),
      topics: context.topics,
      lastQuery: context.lastQuery,
      workspace: context.workspace,
      sessionStart: context.sessionStart
    };

    localStorage.setItem(persistKey, JSON.stringify(toStore));
  }, [context, persistKey]);

  // Update context after a query and response
  const updateFromResponse = useCallback((query: string, references: CodeReference[]) => {
    setContext(prev => {
      const newFilesDiscussed = new Set(prev.filesDiscussed);
      const newFunctionsDiscussed = new Set(prev.functionsDiscussed);
      const newTopics = [...prev.topics];

      // Extract files and functions from references
      references.forEach(ref => {
        if (ref.filePath) {
          newFilesDiscussed.add(ref.filePath);
        }
        if (ref.function) {
          newFunctionsDiscussed.add(ref.function);
        }
      });

      // Extract topics from query (simple keyword extraction)
      const keywords = extractKeywords(query);
      keywords.forEach(keyword => {
        if (!newTopics.includes(keyword)) {
          newTopics.push(keyword);
        }
      });

      // Keep only last 10 topics
      if (newTopics.length > 10) {
        newTopics.splice(0, newTopics.length - 10);
      }

      return {
        ...prev,
        filesDiscussed: newFilesDiscussed,
        functionsDiscussed: newFunctionsDiscussed,
        topics: newTopics,
        lastQuery: query
      };
    });
  }, []);

  // Build contextual prompt prefix
  const buildContextualQuery = useCallback((userQuery: string): string => {
    const parts: string[] = [];

    // Add file context
    if (context.filesDiscussed.size > 0) {
      const files = Array.from(context.filesDiscussed).slice(-3); // Last 3 files
      parts.push(`Context: We're discussing ${files.join(', ')}.`);
    }

    // Add function context
    if (context.functionsDiscussed.size > 0) {
      const funcs = Array.from(context.functionsDiscussed).slice(-3);
      parts.push(`Functions: ${funcs.join(', ')}.`);
    }

    // Add topic context
    if (context.topics.length > 0) {
      const recentTopics = context.topics.slice(-3);
      parts.push(`Recent topics: ${recentTopics.join(', ')}.`);
    }

    // Add last query for follow-up detection
    if (context.lastQuery && isFollowUpQuestion(userQuery)) {
      parts.push(`Previous question: "${context.lastQuery}".`);
    }

    if (parts.length > 0) {
      return `${parts.join(' ')}\n\nCurrent question: ${userQuery}`;
    }

    return userQuery;
  }, [context]);

  // Get most relevant file from context
  const getMostRelevantFile = useCallback((): string | undefined => {
    if (context.filesDiscussed.size === 0) return undefined;
    // Return the most recently discussed file
    return Array.from(context.filesDiscussed).pop();
  }, [context]);

  // Get most relevant function from context
  const getMostRelevantFunction = useCallback((): string | undefined => {
    if (context.functionsDiscussed.size === 0) return undefined;
    return Array.from(context.functionsDiscussed).pop();
  }, [context]);

  // Clear context (new session)
  const clearContext = useCallback(() => {
    setContext({
      filesDiscussed: new Set<string>(),
      functionsDiscussed: new Set<string>(),
      topics: [],
      lastQuery: '',
      workspace,
      sessionStart: Date.now()
    });
  }, [workspace]);

  // Add file manually
  const addFile = useCallback((filePath: string) => {
    setContext(prev => ({
      ...prev,
      filesDiscussed: new Set([...prev.filesDiscussed, filePath])
    }));
  }, []);

  // Add function manually
  const addFunction = useCallback((functionName: string) => {
    setContext(prev => ({
      ...prev,
      functionsDiscussed: new Set([...prev.functionsDiscussed, functionName])
    }));
  }, []);

  return {
    context,
    updateFromResponse,
    buildContextualQuery,
    getMostRelevantFile,
    getMostRelevantFunction,
    clearContext,
    addFile,
    addFunction
  };
}

// Helper: Extract keywords from query
function extractKeywords(query: string): string[] {
  const commonWords = new Set(['the', 'a', 'an', 'in', 'on', 'at', 'to', 'for', 'of', 'with', 'is', 'are', 'was', 'were', 'how', 'what', 'where', 'when', 'why', 'does', 'do', 'can', 'show', 'find', 'explain', 'tell', 'me']);

  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(word => word.length > 3 && !commonWords.has(word))
    .slice(0, 5); // Max 5 keywords
}

// Helper: Detect if query is a follow-up question
function isFollowUpQuestion(query: string): boolean {
  const followUpIndicators = [
    'and', 'also', 'what about', 'how about', 'that', 'this', 'it', 'show me', 'explain more',
    'what else', 'continue', 'more', 'further', 'additionally'
  ];

  const lowerQuery = query.toLowerCase();
  return followUpIndicators.some(indicator => lowerQuery.startsWith(indicator) || lowerQuery.includes(indicator));
}
