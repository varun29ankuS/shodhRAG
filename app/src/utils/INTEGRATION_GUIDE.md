# Intelligent Retrieval & Context Tracking Integration Guide

This guide shows how to integrate the retrieval decision and context accumulator systems into your components.

## Quick Start

### 1. Use the Hook in Your Component

```tsx
import { useIntelligentRetrieval } from './hooks/useIntelligentRetrieval';

function MyChatComponent() {
  const {
    search,
    trackUser,
    trackAssistant,
    lastAnalysis,
    isAnalyzing
  } = useIntelligentRetrieval();

  // ... your component code
}
```

### 2. Replace Direct Search Calls

**Before:**
```tsx
const searchResponse = await invoke("search_documents", {
  query: userQuery,
  spaceId: activeSpaceId,
  maxResults: 5
});
```

**After:**
```tsx
const { decision, results, rewriting } = await search(
  userQuery,
  activeSpaceId,
  5
);

// Now you have:
// - decision.shouldRetrieve: boolean (go/no-go)
// - decision.reasoning: string (why?)
// - results: search results (only if shouldRetrieve = true)
```

### 3. Track User Interactions

```tsx
// When user sends a message
await trackUser(userMessage);

// When assistant responds
await trackAssistant(assistantResponse);

// When user views a document
await trackDocument(docId, docTitle, docType);
```

### 4. Show Context to User

Add the ContextPanel to your UI:

```tsx
import { ContextPanel } from './components/ContextPanel';

function MyLayout() {
  return (
    <div>
      {/* Your existing UI */}
      <ContextPanel /> {/* Shows current context */}
    </div>
  );
}
```

## Complete Example: Chat Component

```tsx
import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useIntelligentRetrieval } from './hooks/useIntelligentRetrieval';

function ChatComponent() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState('');
  const { search, trackUser, trackAssistant, lastAnalysis } = useIntelligentRetrieval();

  const handleSend = async () => {
    const userMessage = input.trim();
    if (!userMessage) return;

    // 1. Add user message to UI
    setMessages(prev => [...prev, {
      role: 'user',
      content: userMessage,
      timestamp: new Date().toISOString()
    }]);

    // 2. Track user message in context
    await trackUser(userMessage);

    // 3. Execute intelligent search
    const { decision, results } = await search(userMessage, null, 5);

    // 4. Show decision to user
    console.log("Retrieval decision:", {
      shouldRetrieve: decision.decision.shouldRetrieve,
      reasoning: decision.decision.reasoning,
      strategy: decision.decision.strategy
    });

    // 5. Generate response based on decision
    let response = '';

    if (decision.decision.shouldRetrieve) {
      // Retrieved documents - use them
      const contextSnippets = results.map(r => r.snippet || '');

      response = await invoke<string>('llm_generate_with_rag', {
        query: userMessage,
        context: contextSnippets
      });

      // Add citations
      response += `\n\n📚 Sources:\n${results.map((r, i) =>
        `[${i + 1}] ${r.citation?.title}: "${r.snippet?.substring(0, 100)}..."`
      ).join('\n')}`;
    } else {
      // No retrieval needed - direct LLM response
      response = await invoke<string>('llm_generate', {
        prompt: userMessage
      });

      // Show why no retrieval was needed
      response += `\n\n💡 ${decision.decision.reasoning}`;
    }

    // 6. Add assistant response to UI
    setMessages(prev => [...prev, {
      role: 'assistant',
      content: response,
      timestamp: new Date().toISOString()
    }]);

    // 7. Track assistant response in context
    await trackAssistant(response);

    setInput('');
  };

  return (
    <div>
      {/* Messages */}
      {messages.map((msg, idx) => (
        <div key={idx}>{msg.content}</div>
      ))}

      {/* Show last analysis */}
      {lastAnalysis && (
        <div className="text-xs text-gray-500">
          Intent: {lastAnalysis.intent} |
          Confidence: {(lastAnalysis.decision.confidence * 100).toFixed(0)}%
        </div>
      )}

      {/* Input */}
      <input
        value={input}
        onChange={e => setInput(e.target.value)}
        onKeyDown={e => e.key === 'Enter' && handleSend()}
      />
    </div>
  );
}
```

## Integration Points in App-SplitView.tsx

### Location 1: handleSendMessage function (line ~657)

Replace the query rewriting call with intelligent search:

```tsx
// OLD:
const searchResponse: any = await invoke("search_with_query_rewriting", {
  query: userQuery,
  spaceId: activeSpaceId || null,
  maxResults: 5
});

// NEW:
import { intelligentSearch, trackUserMessage, trackAssistantMessage } from './utils/intelligentRetrieval';

// Track user message
await trackUserMessage(userQuery);

// Execute intelligent search (includes analysis + tracking)
const { decision, results, rewriting } = await intelligentSearch(
  userQuery,
  activeSpaceId || null,
  5
);

// Only use results if decision says to retrieve
if (decision.decision.shouldRetrieve) {
  const contextSnippets = results.map(r => r.snippet || '');
  // ... use contextSnippets for LLM
} else {
  // Direct LLM response without retrieval
  console.log("Skipping retrieval:", decision.decision.reasoning);
}

// After getting assistant response
await trackAssistantMessage(response);
```

### Location 2: handleSearch function (line ~858)

Same pattern - replace direct search with intelligent search:

```tsx
const { decision, results } = await intelligentSearch(
  searchQuery,
  activeSpaceId || null,
  20
);

setSearchResults(results);

// Show decision to user
if (!decision.decision.shouldRetrieve) {
  console.log("Search not needed:", decision.decision.reasoning);
}
```

### Location 3: Add Context Panel to UI

In the main layout, add the ContextPanel:

```tsx
import { ContextPanel } from './components/ContextPanel';

// In your JSX, add it to the sidebar or settings area:
<div className="sidebar">
  {/* Existing sidebar content */}
  <ContextPanel />
</div>
```

## Key Benefits

1. **Smarter retrieval**: No wasteful searches for greetings, meta-questions, etc.
2. **Better UX**: User sees why retrieval happened (or didn't)
3. **Context awareness**: System remembers conversation flow
4. **Measurable improvements**:
   - 500ms saved per greeting (no retrieval)
   - Higher quality responses (context-aware)
   - Better search refinement suggestions

## Testing

Test with these queries to see the system in action:

1. **Greeting** (should NOT retrieve):
   - "hello"
   - "hi there"
   - Expected: `shouldRetrieve: false`, reasoning about greeting

2. **Factual lookup** (should retrieve):
   - "What is the deadline for the Q4 report?"
   - Expected: `shouldRetrieve: true`, strategy: TopK

3. **Filtered search** (should retrieve with filters):
   - "Show me all contracts signed in December"
   - Expected: `shouldRetrieve: true`, strategy: FilteredSearch

4. **Meta question** (should NOT retrieve):
   - "How do I search for documents?"
   - Expected: `shouldRetrieve: false`, reasoning about system capability

## Troubleshooting

If context tracking isn't working:

```tsx
// Check context summary manually
import { getFullContext } from './utils/intelligentRetrieval';

const context = await getFullContext();
console.log("Current context:", context);
```

If retrieval decisions seem wrong:

```tsx
// Check corpus stats
import { getCorpusStats } from './utils/intelligentRetrieval';

const stats = await getCorpusStats(spaceId);
console.log("Corpus stats:", stats);
// Should show: totalDocs, vocabularySize, documentTypes
```
