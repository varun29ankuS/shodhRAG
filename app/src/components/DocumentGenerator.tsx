/**
 * Document Generator — Enterprise RAG-Grounded Report Generation
 *
 * Flow: Describe → Search Sources → Review & Select → Generate → Preview & Export
 * All generation is grounded in indexed documents. No "generate from knowledge" path.
 */

import { useState, useEffect, useRef, useCallback } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  FileText, Sparkles, Search, Check, Download,
  Copy, ChevronRight, ChevronDown, ArrowLeft, Database, Loader2,
  BookOpen, Settings, CheckSquare, Square,
  RotateCcw, AlertTriangle, X, Eye, ClipboardCopy
} from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";
import { useTheme } from "../contexts/ThemeContext";
import SmartTemplates from "./SmartTemplates";

type GenerationStep = 'input' | 'searching' | 'sources' | 'generating' | 'preview';
type OutputFormat = 'md' | 'pdf' | 'docx' | 'html' | 'txt' | 'xlsx' | 'json';
type DocumentLength = 'Brief' | 'Standard' | 'Detailed' | 'Maximum';

interface Source {
  id: string;
  title: string;
  snippet: string;
  score: number;
  file_path?: string;
  selected: boolean;
}

interface TemplateSection {
  name: string;
  order: number;
  content_type: string;
  placeholder: string;
  is_required: boolean;
  formatting_rules: string[];
}

interface TemplateVariable {
  name: string;
  description: string;
  default_value?: string;
  validation_pattern?: string;
}

interface DocumentTemplate {
  id: string;
  name: string;
  description: string;
  sections: TemplateSection[];
  metadata: { document_type: string; confidence_score: number };
  variables: TemplateVariable[];
  example_content: string;
}

const LENGTH_OPTIONS: { value: DocumentLength; label: string; desc: string }[] = [
  { value: 'Brief', label: 'Brief', desc: '1-2 pages' },
  { value: 'Standard', label: 'Standard', desc: '4-8 pages' },
  { value: 'Detailed', label: 'Detailed', desc: '16-32 pages' },
  { value: 'Maximum', label: 'Maximum', desc: 'Full deep-dive' },
];

const FORMAT_OPTIONS: { value: OutputFormat; label: string }[] = [
  { value: 'md', label: 'MD' },
  { value: 'pdf', label: 'PDF' },
  { value: 'docx', label: 'DOCX' },
  { value: 'html', label: 'HTML' },
  { value: 'txt', label: 'TXT' },
  { value: 'xlsx', label: 'XLSX' },
  { value: 'json', label: 'JSON' },
];

const MIME_TYPES: Record<string, string> = {
  md: 'text/markdown',
  html: 'text/html',
  txt: 'text/plain',
  json: 'application/json',
  pdf: 'application/pdf',
  docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
};

const BINARY_FORMATS = new Set(['pdf', 'docx', 'xlsx']);

export default function DocumentGenerator() {
  const { colors } = useTheme();

  const [step, setStep] = useState<GenerationStep>('input');
  const [prompt, setPrompt] = useState('');
  const [format, setFormat] = useState<OutputFormat>('md');
  const [length, setLength] = useState<DocumentLength>('Standard');
  const [error, setError] = useState('');

  // Templates
  const [templates, setTemplates] = useState<DocumentTemplate[]>([]);
  const [selectedTemplateId, setSelectedTemplateId] = useState('');
  const [selectedTemplateDetail, setSelectedTemplateDetail] = useState<DocumentTemplate | null>(null);
  const [showTemplatePreview, setShowTemplatePreview] = useState(false);
  const [showTemplateExample, setShowTemplateExample] = useState(false);
  const [templateCopied, setTemplateCopied] = useState(false);
  const [showTemplateManager, setShowTemplateManager] = useState(false);

  // RAG sources
  const [sources, setSources] = useState<Source[]>([]);
  const [searchProgress, setSearchProgress] = useState(0);
  const [expandedSourceId, setExpandedSourceId] = useState<string | null>(null);

  // Generation
  const [generatedContent, setGeneratedContent] = useState('');
  const [displayedContent, setDisplayedContent] = useState('');
  const [isRevealing, setIsRevealing] = useState(false);
  const [contentBase64, setContentBase64] = useState('');
  const [generationProgress, setGenerationProgress] = useState(0);
  const [copied, setCopied] = useState(false);

  // Auto-scroll ref
  const previewEndRef = useRef<HTMLDivElement>(null);
  const previewContainerRef = useRef<HTMLDivElement>(null);
  const revealTimerRef = useRef<number | null>(null);

  useEffect(() => {
    loadTemplates();
  }, []);

  // When template selection changes, fetch full details
  useEffect(() => {
    if (selectedTemplateId) {
      fetchTemplateDetail(selectedTemplateId);
    } else {
      setSelectedTemplateDetail(null);
      setShowTemplatePreview(false);
      setShowTemplateExample(false);
    }
  }, [selectedTemplateId]);

  // Typewriter reveal: progressively show content line-by-line
  const startReveal = useCallback((fullContent: string) => {
    setDisplayedContent('');
    setIsRevealing(true);

    const lines = fullContent.split('\n');
    let currentLine = 0;
    // Reveal 1 line at a time at a readable pace
    const linesPerTick = 1;
    const intervalMs = 60;

    const tick = () => {
      currentLine += linesPerTick;
      const visibleLines = lines.slice(0, Math.min(currentLine, lines.length));
      setDisplayedContent(visibleLines.join('\n'));

      if (currentLine >= lines.length) {
        setDisplayedContent(fullContent);
        setIsRevealing(false);
        revealTimerRef.current = null;
      } else {
        revealTimerRef.current = window.setTimeout(tick, intervalMs);
      }
    };

    revealTimerRef.current = window.setTimeout(tick, 80);
  }, []);

  // Auto-scroll to bottom as content reveals
  useEffect(() => {
    if (isRevealing && previewContainerRef.current) {
      previewContainerRef.current.scrollTop = previewContainerRef.current.scrollHeight;
    }
  }, [displayedContent, isRevealing]);

  // Cleanup reveal timer on unmount
  useEffect(() => {
    return () => {
      if (revealTimerRef.current) {
        clearTimeout(revealTimerRef.current);
      }
    };
  }, []);

  const loadTemplates = async () => {
    try {
      const result = await invoke<DocumentTemplate[]>('list_templates');
      setTemplates(result);
    } catch (err) {
      console.error('Failed to load templates:', err);
    }
  };

  const fetchTemplateDetail = async (templateId: string) => {
    try {
      const detail = await invoke<DocumentTemplate>('get_template', { templateId });
      setSelectedTemplateDetail(detail);
      setShowTemplatePreview(true);
    } catch (err) {
      console.error('Failed to fetch template:', err);
      setSelectedTemplateDetail(null);
    }
  };

  const handleSearch = async () => {
    if (!prompt.trim()) return;

    setStep('searching');
    setSearchProgress(0);
    setError('');

    try {
      const progressInterval = setInterval(() => {
        setSearchProgress(prev => Math.min(prev + 12, 90));
      }, 200);

      const response: any = await invoke('search_documents', {
        request: {
          query: prompt,
          max_results: 20,
          filters: null
        }
      });

      clearInterval(progressInterval);
      setSearchProgress(100);

      // response is SearchResponse { results: SearchResult[], decision: DecisionMetadata }
      const resultsList = response.results || [];

      const foundSources: Source[] = resultsList.map((r: any, idx: number) => ({
        id: r.id || `source-${idx}`,
        title: r.citation?.title || r.metadata?.file_name || `Document ${idx + 1}`,
        snippet: r.snippet || r.text || '',
        score: r.score || 0,
        file_path: r.metadata?.file_path || r.sourceFile,
        selected: true
      }));

      setSources(foundSources);
      setTimeout(() => setStep('sources'), 400);

    } catch (err: any) {
      console.error('Search failed:', err);
      setError(`Search failed: ${err.message || err}`);
      setStep('input');
    }
  };

  const handleGenerate = async () => {
    const selectedSources = sources.filter(s => s.selected);
    if (selectedSources.length === 0) return;

    setStep('generating');
    setGenerationProgress(0);
    setError('');

    try {
      const progressInterval = setInterval(() => {
        setGenerationProgress(prev => Math.min(prev + 8, 90));
      }, 400);

      const response: any = await invoke('generate_from_rag', {
        prompt,
        format,
        includeReferences: true,
        maxSourceDocs: selectedSources.length,
        template: selectedTemplateId || null,
        desiredLength: length,
      });

      clearInterval(progressInterval);
      setGenerationProgress(100);

      // Store both text preview and raw base64 for binary downloads
      let content = '';
      if (response.preview) {
        content = response.preview;
      } else if (response.contentBase64) {
        content = atob(response.contentBase64);
      }

      setGeneratedContent(content);
      setContentBase64(response.contentBase64 || '');
      // Switch to preview immediately and start typewriter reveal
      setStep('preview');
      startReveal(content);

    } catch (err: any) {
      console.error('Generation failed:', err);
      setError(`Generation failed: ${err.message || err}`);
      setStep('sources');
    }
  };

  const handleCopy = () => {
    navigator.clipboard.writeText(generatedContent);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleCopyTemplateStructure = () => {
    if (selectedTemplateDetail?.example_content) {
      navigator.clipboard.writeText(selectedTemplateDetail.example_content);
      setTemplateCopied(true);
      setTimeout(() => setTemplateCopied(false), 2000);
    }
  };

  const handleDownload = () => {
    const isBinary = BINARY_FORMATS.has(format);

    if (isBinary && contentBase64) {
      // Decode base64 to binary for PDF/DOCX/XLSX
      const byteCharacters = atob(contentBase64);
      const byteNumbers = new Array(byteCharacters.length);
      for (let i = 0; i < byteCharacters.length; i++) {
        byteNumbers[i] = byteCharacters.charCodeAt(i);
      }
      const byteArray = new Uint8Array(byteNumbers);
      const blob = new Blob([byteArray], { type: MIME_TYPES[format] || 'application/octet-stream' });
      downloadBlob(blob);
    } else {
      // Text formats
      const blob = new Blob([generatedContent], { type: MIME_TYPES[format] || 'text/plain' });
      downloadBlob(blob);
    }
  };

  const downloadBlob = (blob: Blob) => {
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `report-${Date.now()}.${format}`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  };

  const handleReset = () => {
    setStep('input');
    setPrompt('');
    setSelectedTemplateId('');
    setSelectedTemplateDetail(null);
    setShowTemplatePreview(false);
    setShowTemplateExample(false);
    setSources([]);
    setGeneratedContent('');
    setDisplayedContent('');
    setIsRevealing(false);
    if (revealTimerRef.current) { clearTimeout(revealTimerRef.current); revealTimerRef.current = null; }
    setContentBase64('');
    setError('');
    setExpandedSourceId(null);
  };

  const toggleSelectAll = () => {
    const allSelected = sources.every(s => s.selected);
    setSources(sources.map(s => ({ ...s, selected: !allSelected })));
  };

  const selectedCount = sources.filter(s => s.selected).length;

  const steps = [
    { id: 'input', label: 'Describe', icon: FileText },
    { id: 'sources', label: 'Sources', icon: Database },
    { id: 'generating', label: 'Generate', icon: Sparkles },
    { id: 'preview', label: 'Export', icon: BookOpen },
  ];

  const stepIndex = (id: string) => steps.findIndex(s => s.id === id);
  const currentStepIndex = step === 'searching' ? 0 : stepIndex(step);

  return (
    <div className="h-full flex flex-col transition-colors duration-200" style={{ backgroundColor: colors.bg }}>
      {/* Progress Steps */}
      <div className="border-b-2 px-6 py-4" style={{ borderColor: colors.border, backgroundColor: colors.bgSecondary }}>
        <div className="flex items-center justify-between max-w-4xl mx-auto">
          {steps.map((s, idx) => {
            const isActive = (step === 'searching' && idx === 0) || step === s.id;
            const isCompleted = currentStepIndex > idx;
            return (
              <div key={s.id} className="flex items-center">
                <div className="flex items-center gap-2" style={{
                  color: isActive ? colors.primary : isCompleted ? colors.success : colors.textMuted
                }}>
                  <div className="w-8 h-8 rounded-full flex items-center justify-center border-2" style={{
                    borderColor: isActive ? colors.primary : isCompleted ? colors.success : colors.border,
                    backgroundColor: isActive ? `${colors.primary}20` : isCompleted ? `${colors.success}20` : 'transparent'
                  }}>
                    {isCompleted ? <Check className="w-4 h-4" /> : <s.icon className="w-4 h-4" />}
                  </div>
                  <span className="text-sm font-medium hidden sm:inline">{s.label}</span>
                </div>
                {idx < steps.length - 1 && (
                  <ChevronRight className="w-4 h-4 mx-2" style={{ color: colors.border }} />
                )}
              </div>
            );
          })}
        </div>
      </div>

      {/* Content Area */}
      <div className="flex-1 overflow-y-auto p-6">
        <div className="max-w-4xl mx-auto">

          {/* Inline Error Banner */}
          <AnimatePresence>
            {error && (
              <motion.div
                initial={{ opacity: 0, y: -10 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -10 }}
                className="mb-4 p-4 rounded-lg flex items-center justify-between"
                style={{
                  backgroundColor: `${colors.error}15`,
                  border: `1px solid ${colors.error}40`,
                }}
              >
                <div className="flex items-center gap-2">
                  <AlertTriangle className="w-4 h-4 shrink-0" style={{ color: colors.error }} />
                  <span className="text-sm" style={{ color: colors.error }}>{error}</span>
                </div>
                <button
                  onClick={() => setError('')}
                  className="p-1 rounded hover:opacity-80"
                  style={{ color: colors.error }}
                >
                  <X className="w-4 h-4" />
                </button>
              </motion.div>
            )}
          </AnimatePresence>

          <AnimatePresence mode="wait">

            {/* Step 1: Input */}
            {step === 'input' && (
              <motion.div key="input" initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -20 }}>
                <Card style={{ backgroundColor: colors.cardBg, borderColor: colors.cardBorder }}>
                  <CardHeader>
                    <CardTitle className="flex items-center gap-2" style={{ color: colors.text }}>
                      <Sparkles className="w-5 h-5" style={{ color: colors.primary }} />
                      Generate Document from Your Knowledge Base
                    </CardTitle>
                    <p className="text-sm mt-1" style={{ color: colors.textMuted }}>
                      Describe what you need. We'll search your indexed documents, let you review sources, then generate a grounded report with citations.
                    </p>
                  </CardHeader>
                  <CardContent className="space-y-5">

                    {/* Template Selector */}
                    <div>
                      <div className="flex items-center justify-between mb-2">
                        <label className="text-sm font-medium" style={{ color: colors.textMuted }}>REPORT TYPE</label>
                        <button
                          onClick={() => setShowTemplateManager(true)}
                          className="text-xs flex items-center gap-1 px-2 py-1 rounded"
                          style={{ color: colors.primary, backgroundColor: `${colors.primary}10` }}
                        >
                          <Settings className="w-3 h-3" />
                          Manage Templates
                        </button>
                      </div>
                      <select
                        value={selectedTemplateId}
                        onChange={(e) => setSelectedTemplateId(e.target.value)}
                        className="w-full px-4 py-2.5 rounded-lg outline-none"
                        style={{ backgroundColor: colors.inputBg, border: `1px solid ${colors.border}`, color: colors.text }}
                      >
                        <option value="">General Report</option>
                        {templates.map((template) => (
                          <option key={template.id} value={template.id}>
                            {template.name} ({template.sections.length} sections)
                          </option>
                        ))}
                      </select>
                    </div>

                    {/* Template Structure Preview (lawyer feature) */}
                    {selectedTemplateDetail && showTemplatePreview && (
                      <motion.div
                        initial={{ opacity: 0, height: 0 }}
                        animate={{ opacity: 1, height: 'auto' }}
                        exit={{ opacity: 0, height: 0 }}
                        className="rounded-lg border overflow-hidden"
                        style={{ borderColor: `${colors.primary}40`, backgroundColor: `${colors.primary}08` }}
                      >
                        <button
                          onClick={() => setShowTemplatePreview(!showTemplatePreview)}
                          className="w-full px-4 py-3 flex items-center justify-between text-left"
                          style={{ color: colors.text }}
                        >
                          <div className="flex items-center gap-2">
                            <Eye className="w-4 h-4" style={{ color: colors.primary }} />
                            <span className="text-sm font-medium">Template Structure: {selectedTemplateDetail.name}</span>
                          </div>
                          <ChevronDown className="w-4 h-4" style={{ color: colors.textMuted }} />
                        </button>

                        <div className="px-4 pb-4 space-y-3">
                          {/* Sections */}
                          <div>
                            <h4 className="text-xs font-semibold mb-2" style={{ color: colors.textMuted }}>SECTIONS</h4>
                            <div className="space-y-1">
                              {selectedTemplateDetail.sections.map((section, idx) => (
                                <div key={idx} className="flex items-center gap-2 text-sm" style={{ color: colors.text }}>
                                  <span style={{ color: colors.textMuted }}>{idx + 1}.</span>
                                  <span className="font-medium">{section.name}</span>
                                  <span className="text-xs px-1.5 py-0.5 rounded" style={{
                                    backgroundColor: colors.bgTertiary,
                                    color: colors.textMuted,
                                  }}>
                                    {section.content_type}
                                  </span>
                                  {section.is_required && (
                                    <span className="text-xs" style={{ color: colors.error }}>*required</span>
                                  )}
                                </div>
                              ))}
                            </div>
                          </div>

                          {/* Variables */}
                          {selectedTemplateDetail.variables.length > 0 && (
                            <div>
                              <h4 className="text-xs font-semibold mb-2" style={{ color: colors.textMuted }}>
                                VARIABLES ({selectedTemplateDetail.variables.length} fields to fill)
                              </h4>
                              <div className="space-y-1">
                                {selectedTemplateDetail.variables.map((v, idx) => (
                                  <div key={idx} className="text-sm" style={{ color: colors.text }}>
                                    <span className="font-mono text-xs px-1 py-0.5 rounded mr-2" style={{
                                      backgroundColor: colors.bgTertiary,
                                      color: colors.primary,
                                    }}>
                                      {v.name}
                                    </span>
                                    <span style={{ color: colors.textMuted }}>{v.description}</span>
                                  </div>
                                ))}
                              </div>
                            </div>
                          )}

                          {/* Example Content */}
                          {selectedTemplateDetail.example_content && (
                            <div>
                              <button
                                onClick={() => setShowTemplateExample(!showTemplateExample)}
                                className="text-xs flex items-center gap-1 mb-2"
                                style={{ color: colors.primary }}
                              >
                                {showTemplateExample ? <ChevronDown className="w-3 h-3" /> : <ChevronRight className="w-3 h-3" />}
                                {showTemplateExample ? 'Hide Example' : 'Preview Example'}
                              </button>
                              {showTemplateExample && (
                                <div className="rounded-lg p-3 text-xs font-mono whitespace-pre-wrap max-h-48 overflow-y-auto" style={{
                                  backgroundColor: colors.bgTertiary,
                                  color: colors.text,
                                  border: `1px solid ${colors.border}`,
                                }}>
                                  {selectedTemplateDetail.example_content}
                                </div>
                              )}
                            </div>
                          )}

                          {/* Actions */}
                          <div className="flex gap-2 pt-1">
                            <button
                              onClick={handleCopyTemplateStructure}
                              className="text-xs flex items-center gap-1 px-3 py-1.5 rounded-lg transition-all"
                              style={{
                                backgroundColor: colors.bgTertiary,
                                color: templateCopied ? colors.success : colors.text,
                                border: `1px solid ${colors.border}`,
                              }}
                            >
                              {templateCopied ? <Check className="w-3 h-3" /> : <ClipboardCopy className="w-3 h-3" />}
                              {templateCopied ? 'Copied!' : 'Copy Structure'}
                            </button>
                          </div>
                        </div>
                      </motion.div>
                    )}

                    {/* Prompt Input */}
                    <div>
                      <label className="text-sm font-medium mb-2 block" style={{ color: colors.textMuted }}>WHAT DO YOU NEED?</label>
                      <textarea
                        className="w-full h-32 p-4 rounded-lg outline-none resize-none"
                        style={{ backgroundColor: colors.inputBg, border: `1px solid ${colors.border}`, color: colors.text }}
                        onFocus={(e) => e.target.style.borderColor = colors.primary}
                        onBlur={(e) => e.target.style.borderColor = colors.border}
                        placeholder="e.g., Generate a compliance report for Q4 2024 covering all regulatory findings and remediation steps"
                        value={prompt}
                        onChange={(e) => setPrompt(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && prompt.trim()) {
                            handleSearch();
                          }
                        }}
                      />
                      <span className="text-xs mt-1 block" style={{ color: colors.textMuted }}>
                        {prompt.length > 0 ? `${prompt.length} chars` : 'Be specific — the more detail, the better the report'}
                        {' '} &middot; Ctrl+Enter to search
                      </span>
                    </div>

                    {/* Format Selector */}
                    <div>
                      <label className="text-sm font-medium mb-2 block" style={{ color: colors.textMuted }}>OUTPUT FORMAT</label>
                      <div className="flex gap-2 flex-wrap">
                        {FORMAT_OPTIONS.map(({ value, label }) => (
                          <button
                            key={value}
                            className="px-4 py-2 rounded-lg border text-sm font-medium transition-all"
                            style={{
                              backgroundColor: format === value ? colors.primary : 'transparent',
                              borderColor: format === value ? colors.primary : colors.border,
                              color: format === value ? colors.primaryText : colors.textMuted
                            }}
                            onClick={() => setFormat(value)}
                          >
                            {label}
                          </button>
                        ))}
                      </div>
                    </div>

                    {/* Document Length Selector */}
                    <div>
                      <label className="text-sm font-medium mb-2 block" style={{ color: colors.textMuted }}>DOCUMENT LENGTH</label>
                      <div className="flex gap-2 flex-wrap">
                        {LENGTH_OPTIONS.map(({ value, label, desc }) => (
                          <button
                            key={value}
                            className="px-4 py-2 rounded-lg border text-sm transition-all text-left"
                            style={{
                              backgroundColor: length === value ? colors.primary : 'transparent',
                              borderColor: length === value ? colors.primary : colors.border,
                              color: length === value ? colors.primaryText : colors.textMuted,
                            }}
                            onClick={() => setLength(value)}
                          >
                            <span className="font-medium">{label}</span>
                            <span className="ml-1 opacity-70 text-xs">({desc})</span>
                          </button>
                        ))}
                      </div>
                    </div>

                    {/* Search Button */}
                    <motion.button
                      whileHover={{ scale: 1.01 }}
                      whileTap={{ scale: 0.99 }}
                      onClick={handleSearch}
                      disabled={!prompt.trim()}
                      className="w-full py-3 rounded-lg font-medium flex items-center justify-center gap-2 disabled:opacity-50 disabled:cursor-not-allowed"
                      style={{ background: `linear-gradient(to right, ${colors.primary}, ${colors.accent || colors.primary})`, color: colors.primaryText }}
                    >
                      <Search className="w-5 h-5" />
                      Search Knowledge Base
                    </motion.button>
                  </CardContent>
                </Card>
              </motion.div>
            )}

            {/* Step 2: Searching */}
            {step === 'searching' && (
              <motion.div key="searching" initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -20 }}>
                <Card style={{ backgroundColor: colors.cardBg, borderColor: colors.cardBorder }}>
                  <CardContent className="p-12 text-center">
                    <Loader2 className="w-12 h-12 animate-spin mx-auto mb-4" style={{ color: colors.primary }} />
                    <h3 className="text-xl font-semibold mb-2" style={{ color: colors.text }}>Searching Knowledge Base</h3>
                    <p className="mb-6" style={{ color: colors.textMuted }}>Finding relevant documents for your report...</p>
                    <div className="w-full rounded-full h-2 overflow-hidden" style={{ backgroundColor: colors.bgTertiary }}>
                      <motion.div
                        className="h-full"
                        style={{ background: `linear-gradient(to right, ${colors.primary}, ${colors.accent || colors.primary})` }}
                        initial={{ width: 0 }}
                        animate={{ width: `${searchProgress}%` }}
                      />
                    </div>
                  </CardContent>
                </Card>
              </motion.div>
            )}

            {/* Step 3: Source Selection */}
            {step === 'sources' && (
              <motion.div key="sources" initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -20 }} className="space-y-4">
                <Card style={{ backgroundColor: colors.cardBg, borderColor: colors.cardBorder }}>
                  <CardHeader>
                    <div className="flex items-center justify-between">
                      <CardTitle className="flex items-center gap-2" style={{ color: colors.text }}>
                        <Database className="w-5 h-5" style={{ color: colors.success }} />
                        {sources.length} Sources Found
                      </CardTitle>
                      <div className="flex items-center gap-2">
                        <button
                          onClick={toggleSelectAll}
                          className="text-xs flex items-center gap-1 px-2 py-1 rounded"
                          style={{ color: colors.primary, backgroundColor: `${colors.primary}10` }}
                        >
                          {sources.every(s => s.selected) ? <CheckSquare className="w-3 h-3" /> : <Square className="w-3 h-3" />}
                          {sources.every(s => s.selected) ? 'Deselect All' : 'Select All'}
                        </button>
                        <button
                          onClick={() => setStep('input')}
                          className="text-xs flex items-center gap-1 px-2 py-1 rounded"
                          style={{ color: colors.textMuted }}
                        >
                          <ArrowLeft className="w-3 h-3" />
                          Back
                        </button>
                      </div>
                    </div>
                    <p className="text-sm mt-1" style={{ color: colors.textMuted }}>
                      {selectedCount} of {sources.length} selected — deselect irrelevant sources for a more focused report
                    </p>
                  </CardHeader>
                  <CardContent className="space-y-2 max-h-[400px] overflow-y-auto">
                    {sources.length === 0 && (
                      <div className="flex items-center gap-3 p-6 text-center" style={{ color: colors.textMuted }}>
                        <AlertTriangle className="w-5 h-5 shrink-0" />
                        <div>
                          <p className="font-medium" style={{ color: colors.text }}>No matching documents found</p>
                          <p className="text-sm mt-1">Try broadening your search terms or check which folders are indexed.</p>
                        </div>
                      </div>
                    )}
                    {sources.map((source, idx) => {
                      const isExpanded = expandedSourceId === source.id;
                      return (
                        <motion.div
                          key={source.id}
                          initial={{ opacity: 0, x: -10 }}
                          animate={{ opacity: 1, x: 0 }}
                          transition={{ delay: idx * 0.03 }}
                          className="p-3 rounded-lg border cursor-pointer transition-all"
                          style={{
                            backgroundColor: source.selected ? `${colors.primary}08` : 'transparent',
                            borderColor: source.selected ? colors.primary : colors.border,
                            opacity: source.selected ? 1 : 0.6
                          }}
                          onClick={() => {
                            setSources(sources.map(s =>
                              s.id === source.id ? { ...s, selected: !s.selected } : s
                            ));
                          }}
                        >
                          <div className="flex items-start gap-3">
                            <div className="w-4 h-4 rounded border-2 flex items-center justify-center mt-0.5 shrink-0" style={{
                              backgroundColor: source.selected ? colors.primary : 'transparent',
                              borderColor: source.selected ? colors.primary : colors.border
                            }}>
                              {source.selected && <Check className="w-2.5 h-2.5" style={{ color: colors.primaryText }} />}
                            </div>
                            <div className="flex-1 min-w-0">
                              <div className="flex items-center gap-2 mb-0.5">
                                <span className="font-medium text-sm truncate" style={{ color: colors.text }}>{source.title}</span>
                                <span className="text-xs px-1.5 py-0.5 rounded shrink-0" style={{
                                  backgroundColor: source.score > 0.5 ? `${colors.success}20` : `${colors.warning || '#f59e0b'}20`,
                                  color: source.score > 0.5 ? colors.success : (colors.warning || '#f59e0b')
                                }}>
                                  {(source.score * 100).toFixed(0)}%
                                </span>
                              </div>
                              <p className={`text-xs ${isExpanded ? '' : 'line-clamp-2'}`} style={{ color: colors.textMuted }}>
                                {source.snippet}
                              </p>
                              {source.snippet.length > 150 && (
                                <button
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    setExpandedSourceId(isExpanded ? null : source.id);
                                  }}
                                  className="text-xs mt-1 hover:underline"
                                  style={{ color: colors.primary }}
                                >
                                  {isExpanded ? 'Show less' : 'Show more'}
                                </button>
                              )}
                              {source.file_path && (
                                <p className="text-xs mt-0.5 truncate" style={{ color: colors.textMuted, opacity: 0.7 }}>{source.file_path}</p>
                              )}
                            </div>
                          </div>
                        </motion.div>
                      );
                    })}
                  </CardContent>
                </Card>

                <motion.button
                  whileHover={{ scale: 1.01 }}
                  whileTap={{ scale: 0.99 }}
                  onClick={handleGenerate}
                  disabled={selectedCount === 0}
                  className="w-full py-3 rounded-lg font-medium flex items-center justify-center gap-2 disabled:opacity-50 disabled:cursor-not-allowed"
                  style={{ background: `linear-gradient(to right, ${colors.primary}, ${colors.accent || colors.primary})`, color: colors.primaryText }}
                >
                  <Sparkles className="w-5 h-5" />
                  Generate {length} Report from {selectedCount} Source{selectedCount !== 1 ? 's' : ''}
                </motion.button>
              </motion.div>
            )}

            {/* Step 4: Generating */}
            {step === 'generating' && (
              <motion.div key="generating" initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -20 }}>
                <Card style={{ backgroundColor: colors.cardBg, borderColor: colors.cardBorder }}>
                  <CardContent className="p-12 text-center">
                    <Loader2 className="w-12 h-12 animate-spin mx-auto mb-4" style={{ color: colors.primary }} />
                    <h3 className="text-xl font-semibold mb-2" style={{ color: colors.text }}>Generating {length} Report</h3>
                    <p className="mb-6" style={{ color: colors.textMuted }}>
                      Synthesizing {selectedCount} source document{selectedCount !== 1 ? 's' : ''} into a {length.toLowerCase()} report...
                    </p>
                    <div className="w-full rounded-full h-2 overflow-hidden" style={{ backgroundColor: colors.bgTertiary }}>
                      <motion.div
                        className="h-full"
                        style={{ background: `linear-gradient(to right, ${colors.primary}, ${colors.accent || colors.success})` }}
                        initial={{ width: 0 }}
                        animate={{ width: `${generationProgress}%` }}
                      />
                    </div>
                    <p className="text-xs mt-3" style={{ color: colors.textMuted }}>
                      {length === 'Brief' ? 'This should take 10-20 seconds' :
                       length === 'Standard' ? 'This may take 30-60 seconds' :
                       length === 'Detailed' ? 'This may take 1-3 minutes' :
                       'This may take several minutes for a full deep-dive'}
                    </p>
                  </CardContent>
                </Card>
              </motion.div>
            )}

            {/* Step 5: Preview & Export */}
            {step === 'preview' && (
              <motion.div key="preview" initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -20 }} className="space-y-4">
                <Card style={{ backgroundColor: colors.cardBg, borderColor: colors.cardBorder }}>
                  <CardHeader>
                    <div className="flex items-center justify-between">
                      <div>
                        <CardTitle className="flex items-center gap-2" style={{ color: colors.text }}>
                          <Check className="w-5 h-5" style={{ color: colors.success }} />
                          Report Generated
                        </CardTitle>
                        <p className="text-sm mt-1" style={{ color: colors.textMuted }}>
                          {generatedContent.length.toLocaleString()} characters &middot; {format.toUpperCase()} &middot; {selectedCount} sources &middot; {length}
                        </p>
                      </div>
                      <div className="flex gap-2">
                        <button
                          onClick={handleCopy}
                          disabled={isRevealing}
                          className="px-3 py-2 rounded-lg text-sm flex items-center gap-1.5 disabled:opacity-40"
                          style={{ backgroundColor: colors.bgTertiary, color: colors.text }}
                        >
                          {copied ? <Check className="w-4 h-4" style={{ color: colors.success }} /> : <Copy className="w-4 h-4" />}
                          {copied ? 'Copied' : 'Copy'}
                        </button>
                        <button
                          onClick={handleDownload}
                          disabled={isRevealing}
                          className="px-3 py-2 rounded-lg text-sm flex items-center gap-1.5 disabled:opacity-40"
                          style={{ backgroundColor: colors.primary, color: colors.primaryText }}
                        >
                          <Download className="w-4 h-4" />
                          {isRevealing ? 'Rendering...' : `Download ${format.toUpperCase()}`}
                        </button>
                      </div>
                    </div>
                  </CardHeader>
                  <CardContent>
                    <div
                      ref={previewContainerRef}
                      className="rounded-lg p-6 max-h-[600px] overflow-y-auto scroll-smooth"
                      style={{ backgroundColor: colors.bgTertiary }}
                    >
                      {(['md', 'txt', 'html'].includes(format)) ? (
                        <div className="prose prose-sm dark:prose-invert max-w-none" style={{ color: colors.text }}>
                          <ReactMarkdown remarkPlugins={[remarkGfm]}>
                            {isRevealing ? displayedContent : generatedContent}
                          </ReactMarkdown>
                          {isRevealing && (
                            <div className="flex items-center gap-2 mt-4 pb-2">
                              <Loader2 className="w-3 h-3 animate-spin" style={{ color: colors.primary }} />
                              <span className="text-xs" style={{ color: colors.textMuted }}>Rendering document...</span>
                            </div>
                          )}
                        </div>
                      ) : format === 'json' ? (
                        <pre className="text-sm whitespace-pre-wrap font-mono" style={{ color: colors.text }}>
                          {isRevealing ? displayedContent : generatedContent}
                        </pre>
                      ) : (
                        <div className="text-center py-8" style={{ color: colors.textMuted }}>
                          <FileText className="w-12 h-12 mx-auto mb-3" style={{ color: colors.primary }} />
                          <p className="font-medium" style={{ color: colors.text }}>
                            {format.toUpperCase()} file generated successfully
                          </p>
                          <p className="text-sm mt-1">Click "Download {format.toUpperCase()}" to save the file.</p>
                        </div>
                      )}
                      <div ref={previewEndRef} />
                    </div>
                  </CardContent>
                </Card>

                <div className="flex gap-3">
                  <motion.button
                    whileHover={{ scale: 1.01 }}
                    whileTap={{ scale: 0.99 }}
                    onClick={handleReset}
                    className="flex-1 py-3 rounded-lg font-medium flex items-center justify-center gap-2"
                    style={{ backgroundColor: colors.bgTertiary, color: colors.text }}
                  >
                    <RotateCcw className="w-4 h-4" />
                    New Report
                  </motion.button>
                  <motion.button
                    whileHover={{ scale: 1.01 }}
                    whileTap={{ scale: 0.99 }}
                    onClick={() => setStep('sources')}
                    className="flex-1 py-3 rounded-lg font-medium flex items-center justify-center gap-2"
                    style={{ backgroundColor: colors.bgTertiary, color: colors.text }}
                  >
                    <ArrowLeft className="w-4 h-4" />
                    Adjust Sources & Regenerate
                  </motion.button>
                </div>
              </motion.div>
            )}

          </AnimatePresence>
        </div>
      </div>

      {/* Template Manager Modal */}
      {showTemplateManager && (
        <div
          className="fixed inset-0 bg-black/80 flex items-center justify-center p-4 z-50"
          onClick={() => { setShowTemplateManager(false); loadTemplates(); }}
        >
          <motion.div
            initial={{ opacity: 0, scale: 0.95 }}
            animate={{ opacity: 1, scale: 1 }}
            className="w-full max-w-6xl max-h-[90vh] overflow-auto rounded-lg"
            style={{ backgroundColor: colors.bg }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="sticky top-0 z-10 px-6 py-4 border-b-2 flex items-center justify-between" style={{
              backgroundColor: colors.bgSecondary, borderColor: colors.border
            }}>
              <h2 className="text-xl font-bold" style={{ color: colors.text }}>Template Manager</h2>
              <button
                onClick={() => { setShowTemplateManager(false); loadTemplates(); }}
                className="px-4 py-2 rounded-lg"
                style={{ backgroundColor: colors.bgTertiary, color: colors.text }}
              >
                Close
              </button>
            </div>
            <div className="p-6">
              <SmartTemplates />
            </div>
          </motion.div>
        </div>
      )}
    </div>
  );
}
