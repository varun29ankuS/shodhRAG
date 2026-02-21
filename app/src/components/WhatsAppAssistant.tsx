import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "./ui/button";
import { Card } from "./ui/card";
import { Bot, Copy, Send, Loader2, X, Sparkles } from "lucide-react";
import { useTheme } from "../contexts/ThemeContext";

interface AssistantMessage {
  question: string;
  answer: string;
  sources: string[];
  timestamp: Date;
}

export function WhatsAppAssistant() {
  const { colors } = useTheme();
  const [isOpen, setIsOpen] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [currentMessage, setCurrentMessage] = useState<AssistantMessage | null>(null);
  const [selectedText, setSelectedText] = useState("");

  // Listen for selected text from WhatsApp
  useEffect(() => {
    const handleSelection = () => {
      const selection = window.getSelection()?.toString();
      if (selection && selection.length > 3) {
        setSelectedText(selection);
      }
    };

    document.addEventListener('mouseup', handleSelection);
    return () => document.removeEventListener('mouseup', handleSelection);
  }, []);

  const handleAskShodh = async () => {
    if (!selectedText) return;

    setIsLoading(true);
    setIsOpen(true);

    try {
      // Query RAG with the selected message
      const response = await invoke<any>("search_documents", {
        query: selectedText,
        limit: 3
      });

      // Extract answer from top results
      const answer = response.results?.[0]?.content || "I couldn't find relevant information in your knowledge base.";
      const sources = response.results?.map((r: any) => r.metadata?.file_name || "Unknown source") || [];

      setCurrentMessage({
        question: selectedText,
        answer: answer,
        sources: sources,
        timestamp: new Date()
      });
    } catch (error) {
      console.error("Failed to query RAG:", error);
      setCurrentMessage({
        question: selectedText,
        answer: "Error querying knowledge base. Please try again.",
        sources: [],
        timestamp: new Date()
      });
    } finally {
      setIsLoading(false);
    }
  };

  const handleCopyAnswer = () => {
    if (currentMessage) {
      navigator.clipboard.writeText(currentMessage.answer);
    }
  };

  const handleSendReply = () => {
    // This would paste into WhatsApp's input field
    // For now, just copy to clipboard
    handleCopyAnswer();
    alert("Answer copied! Paste it into WhatsApp chat.");
  };

  if (!isOpen && selectedText.length > 10) {
    return (
      <div className="fixed bottom-6 right-6 z-50">
        <Button
          onClick={handleAskShodh}
          className="shadow-lg"
          style={{
            backgroundColor: "#25D366",
            color: "white"
          }}
        >
          <Bot className="w-4 h-4 mr-2" />
          Ask Shodh about this
        </Button>
      </div>
    );
  }

  if (!isOpen) return null;

  return (
    <div className="fixed bottom-6 right-6 w-96 z-50">
      <Card className="shadow-2xl border-2" style={{
        backgroundColor: colors.cardBg,
        borderColor: "#25D366"
      }}>
        {/* Header */}
        <div className="flex items-center justify-between p-4 border-b" style={{
          borderColor: colors.border,
          backgroundColor: "#25D366"
        }}>
          <div className="flex items-center gap-2">
            <Bot className="w-5 h-5 text-white" />
            <span className="font-semibold text-white">Shodh Assistant</span>
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setIsOpen(false)}
            className="h-6 w-6 p-0 text-white hover:bg-white/20"
          >
            <X className="w-4 h-4" />
          </Button>
        </div>

        {/* Content */}
        <div className="p-4 max-h-96 overflow-y-auto">
          {isLoading ? (
            <div className="flex items-center justify-center py-8">
              <Loader2 className="w-8 h-8 animate-spin" style={{ color: colors.primary }} />
              <span className="ml-3" style={{ color: colors.textMuted }}>
                Searching knowledge base...
              </span>
            </div>
          ) : currentMessage ? (
            <div className="space-y-4">
              {/* Question */}
              <div>
                <div className="text-xs font-semibold mb-1" style={{ color: colors.textMuted }}>
                  Question:
                </div>
                <div className="p-3 rounded-lg" style={{
                  backgroundColor: colors.bg,
                  borderColor: colors.border,
                  border: '1px solid'
                }}>
                  <p className="text-sm" style={{ color: colors.text }}>
                    {currentMessage.question}
                  </p>
                </div>
              </div>

              {/* Answer */}
              <div>
                <div className="flex items-center gap-2 mb-2">
                  <Sparkles className="w-4 h-4" style={{ color: "#25D366" }} />
                  <div className="text-xs font-semibold" style={{ color: colors.text }}>
                    Answer from your knowledge:
                  </div>
                </div>
                <div className="p-3 rounded-lg" style={{
                  backgroundColor: "#25D366",
                  color: "white"
                }}>
                  <p className="text-sm">
                    {currentMessage.answer}
                  </p>
                </div>
              </div>

              {/* Sources */}
              {currentMessage.sources.length > 0 && (
                <div>
                  <div className="text-xs font-semibold mb-1" style={{ color: colors.textMuted }}>
                    Sources:
                  </div>
                  <div className="space-y-1">
                    {currentMessage.sources.slice(0, 3).map((source, idx) => (
                      <div key={idx} className="text-xs px-2 py-1 rounded" style={{
                        backgroundColor: colors.bg,
                        color: colors.textMuted
                      }}>
                        • {source}
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {/* Actions */}
              <div className="flex gap-2 pt-2">
                <Button
                  onClick={handleCopyAnswer}
                  variant="outline"
                  size="sm"
                  className="flex-1"
                >
                  <Copy className="w-4 h-4 mr-2" />
                  Copy Answer
                </Button>
                <Button
                  onClick={handleSendReply}
                  size="sm"
                  className="flex-1"
                  style={{
                    backgroundColor: "#25D366",
                    color: "white"
                  }}
                >
                  <Send className="w-4 h-4 mr-2" />
                  Send Reply
                </Button>
              </div>
            </div>
          ) : (
            <div className="text-center py-8">
              <Bot className="w-12 h-12 mx-auto mb-3" style={{ color: colors.textMuted }} />
              <p className="text-sm" style={{ color: colors.textMuted }}>
                Select a message in WhatsApp and click "Ask Shodh"
              </p>
            </div>
          )}
        </div>
      </Card>
    </div>
  );
}
