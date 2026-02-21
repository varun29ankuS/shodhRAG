import React, { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Bot, Plus, Trash2, Save, X, Wand2, Settings2,
  ChevronDown, ChevronRight, ToggleLeft, ToggleRight,
  Search, FileText, Code, Zap, Sparkles, Brain,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface AgentDefinition {
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  enabled: boolean;
  config: AgentConfig;
  capabilities: string[];
  tools: string[];
  metadata: Record<string, string>;
}

interface AgentConfig {
  temperature: number;
  max_tokens: number;
  top_p: number;
  stream: boolean;
  max_tool_calls: number;
  timeout_seconds: number;
  auto_use_rag: boolean;
  rag_top_k: number;
}

interface AgentBuilderProps {
  isOpen: boolean;
  onClose: () => void;
  colors: Record<string, string>;
  theme: string;
  onAgentCreated?: (agent: AgentDefinition) => void;
  editAgent?: AgentDefinition | null;
}

const CAPABILITY_OPTIONS = [
  { id: 'RAGSearch', label: 'RAG Search', icon: Search, desc: 'Search the knowledge base' },
  { id: 'CodeGeneration', label: 'Code Generation', icon: Code, desc: 'Generate and analyze code' },
  { id: 'ToolUse', label: 'Tool Use', icon: Wand2, desc: 'Use external tools and APIs' },
  { id: 'Analysis', label: 'Analysis', icon: Brain, desc: 'Deep analysis and reasoning' },
  { id: 'Summarization', label: 'Summarization', icon: FileText, desc: 'Summarize documents' },
  { id: 'Creative', label: 'Creative Writing', icon: Sparkles, desc: 'Creative content generation' },
];

const PRESET_AGENTS = [
  {
    name: 'Research Assistant',
    description: 'Deep research agent that searches documents, compares sources, and synthesizes findings.',
    system_prompt: 'You are a meticulous research assistant. When given a question:\n1. Search the knowledge base thoroughly\n2. Cross-reference multiple sources\n3. Synthesize findings with proper citations\n4. Highlight any contradictions or gaps',
    capabilities: ['RAGSearch', 'Analysis', 'Summarization'],
  },
  {
    name: 'Code Reviewer',
    description: 'Reviews code for bugs, security issues, and best practices.',
    system_prompt: 'You are an expert code reviewer. Analyze code for:\n- Security vulnerabilities (OWASP top 10)\n- Performance issues\n- Best practice violations\n- Code clarity and maintainability\nProvide specific line-level feedback with fixes.',
    capabilities: ['CodeGeneration', 'Analysis'],
  },
  {
    name: 'Document Drafter',
    description: 'Drafts professional documents using knowledge base context.',
    system_prompt: 'You are a professional document writer. Use the knowledge base to draft well-structured documents. Always cite your sources. Match the tone and style appropriate to the document type.',
    capabilities: ['RAGSearch', 'Summarization', 'Creative'],
  },
];

const defaultConfig: AgentConfig = {
  temperature: 0.7,
  max_tokens: 4096,
  top_p: 0.95,
  stream: true,
  max_tool_calls: 10,
  timeout_seconds: 60,
  auto_use_rag: true,
  rag_top_k: 5,
};

export const AgentBuilder: React.FC<AgentBuilderProps> = ({
  isOpen, onClose, colors, theme, onAgentCreated, editAgent,
}) => {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [systemPrompt, setSystemPrompt] = useState('');
  const [capabilities, setCapabilities] = useState<string[]>([]);
  const [config, setConfig] = useState<AgentConfig>({ ...defaultConfig });
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Initialize with edit data if provided
  useEffect(() => {
    if (editAgent) {
      setName(editAgent.name);
      setDescription(editAgent.description);
      setSystemPrompt(editAgent.system_prompt);
      setCapabilities(editAgent.capabilities);
      setConfig(editAgent.config);
    } else {
      setName('');
      setDescription('');
      setSystemPrompt('');
      setCapabilities([]);
      setConfig({ ...defaultConfig });
    }
  }, [editAgent, isOpen]);

  const toggleCapability = (cap: string) => {
    setCapabilities(prev =>
      prev.includes(cap) ? prev.filter(c => c !== cap) : [...prev, cap]
    );
  };

  const applyPreset = (preset: typeof PRESET_AGENTS[0]) => {
    setName(preset.name);
    setDescription(preset.description);
    setSystemPrompt(preset.system_prompt);
    setCapabilities(preset.capabilities);
  };

  const handleSave = async () => {
    if (!name.trim()) {
      setError('Agent name is required');
      return;
    }
    if (!systemPrompt.trim()) {
      setError('System prompt is required');
      return;
    }

    setSaving(true);
    setError(null);

    try {
      const agentDef: AgentDefinition = {
        id: editAgent?.id || '',
        name: name.trim(),
        description: description.trim(),
        system_prompt: systemPrompt.trim(),
        enabled: true,
        config,
        capabilities,
        tools: [],
        metadata: {},
      };

      if (editAgent?.id) {
        await invoke('update_agent', { agentId: editAgent.id, definition: agentDef });
      } else {
        const agentId = await invoke<string>('create_agent', { definition: agentDef });
        agentDef.id = agentId;
      }

      onAgentCreated?.(agentDef);
      onClose();
    } catch (e: any) {
      setError(e?.toString() || 'Failed to save agent');
    } finally {
      setSaving(false);
    }
  };

  if (!isOpen) return null;

  const inputStyle = {
    background: theme === 'dark' ? 'rgba(255,255,255,0.05)' : 'rgba(0,0,0,0.03)',
    border: `1px solid ${theme === 'dark' ? 'rgba(255,255,255,0.1)' : 'rgba(0,0,0,0.1)'}`,
    color: colors.text,
  };

  return (
    <AnimatePresence>
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        className="fixed inset-0 z-50 flex items-center justify-center"
        style={{ background: 'rgba(0,0,0,0.5)', backdropFilter: 'blur(4px)' }}
        onClick={(e) => { if (e.target === e.currentTarget) onClose(); }}
      >
        <motion.div
          initial={{ opacity: 0, scale: 0.95, y: 20 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.95, y: 20 }}
          className="w-full max-w-2xl max-h-[85vh] rounded-2xl overflow-hidden flex flex-col"
          style={{ background: colors.bgPrimary, border: `1px solid ${colors.border}` }}
        >
          {/* Header */}
          <div className="flex items-center justify-between px-6 py-4" style={{ borderBottom: `1px solid ${colors.border}` }}>
            <div className="flex items-center gap-3">
              <div className="w-8 h-8 rounded-lg flex items-center justify-center" style={{ background: colors.primary + '20' }}>
                <Bot className="w-4 h-4" style={{ color: colors.primary }} />
              </div>
              <div>
                <h2 className="text-base font-semibold" style={{ color: colors.text }}>
                  {editAgent ? 'Edit Agent' : 'Create Agent'}
                </h2>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  Define a custom AI agent with specific behavior
                </p>
              </div>
            </div>
            <button onClick={onClose} className="p-1.5 rounded-lg hover:opacity-70 transition-opacity">
              <X className="w-4 h-4" style={{ color: colors.textMuted }} />
            </button>
          </div>

          {/* Content */}
          <div className="flex-1 overflow-y-auto px-6 py-4 space-y-5">

            {/* Preset Templates */}
            {!editAgent && (
              <div>
                <label className="text-[11px] font-semibold uppercase tracking-wider mb-2 block" style={{ color: colors.textMuted }}>
                  Quick Start Templates
                </label>
                <div className="grid grid-cols-3 gap-2">
                  {PRESET_AGENTS.map((preset, idx) => (
                    <button
                      key={idx}
                      onClick={() => applyPreset(preset)}
                      className="text-left p-3 rounded-lg transition-all hover:scale-[1.02]"
                      style={{
                        background: theme === 'dark' ? 'rgba(255,255,255,0.04)' : 'rgba(0,0,0,0.02)',
                        border: `1px solid ${colors.border}`,
                      }}
                    >
                      <span className="text-xs font-semibold block mb-1" style={{ color: colors.text }}>
                        {preset.name}
                      </span>
                      <span className="text-[10px] line-clamp-2" style={{ color: colors.textMuted }}>
                        {preset.description}
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            )}

            {/* Name */}
            <div>
              <label className="text-[11px] font-semibold uppercase tracking-wider mb-1.5 block" style={{ color: colors.textMuted }}>
                Name
              </label>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. Research Assistant"
                className="w-full px-3 py-2 rounded-lg text-sm outline-none focus:ring-2"
                style={{ ...inputStyle, '--tw-ring-color': colors.primary } as any}
              />
            </div>

            {/* Description */}
            <div>
              <label className="text-[11px] font-semibold uppercase tracking-wider mb-1.5 block" style={{ color: colors.textMuted }}>
                Description
              </label>
              <input
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="What does this agent do?"
                className="w-full px-3 py-2 rounded-lg text-sm outline-none focus:ring-2"
                style={{ ...inputStyle, '--tw-ring-color': colors.primary } as any}
              />
            </div>

            {/* System Prompt */}
            <div>
              <label className="text-[11px] font-semibold uppercase tracking-wider mb-1.5 block" style={{ color: colors.textMuted }}>
                System Prompt
              </label>
              <textarea
                value={systemPrompt}
                onChange={(e) => setSystemPrompt(e.target.value)}
                placeholder="Define the agent's behavior, personality, and instructions..."
                rows={6}
                className="w-full px-3 py-2 rounded-lg text-sm outline-none resize-y focus:ring-2"
                style={{ ...inputStyle, '--tw-ring-color': colors.primary, fontFamily: 'monospace', fontSize: '12px' } as any}
              />
            </div>

            {/* Capabilities */}
            <div>
              <label className="text-[11px] font-semibold uppercase tracking-wider mb-2 block" style={{ color: colors.textMuted }}>
                Capabilities
              </label>
              <div className="grid grid-cols-2 gap-2">
                {CAPABILITY_OPTIONS.map((cap) => {
                  const Icon = cap.icon;
                  const isSelected = capabilities.includes(cap.id);
                  return (
                    <button
                      key={cap.id}
                      onClick={() => toggleCapability(cap.id)}
                      className="flex items-center gap-2.5 p-2.5 rounded-lg transition-all"
                      style={{
                        background: isSelected
                          ? colors.primary + '15'
                          : theme === 'dark' ? 'rgba(255,255,255,0.03)' : 'rgba(0,0,0,0.02)',
                        border: `1px solid ${isSelected ? colors.primary + '40' : colors.border}`,
                      }}
                    >
                      <Icon className="w-3.5 h-3.5 flex-shrink-0" style={{ color: isSelected ? colors.primary : colors.textMuted }} />
                      <div className="text-left">
                        <span className="text-xs font-medium block" style={{ color: isSelected ? colors.primary : colors.text }}>
                          {cap.label}
                        </span>
                        <span className="text-[10px]" style={{ color: colors.textMuted }}>{cap.desc}</span>
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>

            {/* Advanced Settings */}
            <div>
              <button
                onClick={() => setShowAdvanced(!showAdvanced)}
                className="flex items-center gap-2 text-xs font-medium hover:opacity-70 transition-opacity"
                style={{ color: colors.textMuted }}
              >
                <Settings2 className="w-3.5 h-3.5" />
                Advanced Settings
                {showAdvanced ? <ChevronDown className="w-3 h-3" /> : <ChevronRight className="w-3 h-3" />}
              </button>

              <AnimatePresence>
                {showAdvanced && (
                  <motion.div
                    initial={{ height: 0, opacity: 0 }}
                    animate={{ height: 'auto', opacity: 1 }}
                    exit={{ height: 0, opacity: 0 }}
                    className="overflow-hidden"
                  >
                    <div className="mt-3 grid grid-cols-2 gap-3">
                      {/* Temperature */}
                      <div>
                        <label className="text-[10px] font-medium mb-1 block" style={{ color: colors.textMuted }}>
                          Temperature: {config.temperature.toFixed(1)}
                        </label>
                        <input
                          type="range"
                          min="0"
                          max="2"
                          step="0.1"
                          value={config.temperature}
                          onChange={(e) => setConfig(c => ({ ...c, temperature: parseFloat(e.target.value) }))}
                          className="w-full h-1 rounded-full appearance-none cursor-pointer"
                          style={{ accentColor: colors.primary }}
                        />
                      </div>

                      {/* Max Tokens */}
                      <div>
                        <label className="text-[10px] font-medium mb-1 block" style={{ color: colors.textMuted }}>
                          Max Tokens
                        </label>
                        <input
                          type="number"
                          value={config.max_tokens}
                          onChange={(e) => setConfig(c => ({ ...c, max_tokens: parseInt(e.target.value) || 4096 }))}
                          className="w-full px-2 py-1.5 rounded text-xs outline-none"
                          style={inputStyle}
                        />
                      </div>

                      {/* Max Tool Calls */}
                      <div>
                        <label className="text-[10px] font-medium mb-1 block" style={{ color: colors.textMuted }}>
                          Max Tool Calls
                        </label>
                        <input
                          type="number"
                          value={config.max_tool_calls}
                          onChange={(e) => setConfig(c => ({ ...c, max_tool_calls: parseInt(e.target.value) || 10 }))}
                          className="w-full px-2 py-1.5 rounded text-xs outline-none"
                          style={inputStyle}
                        />
                      </div>

                      {/* Timeout */}
                      <div>
                        <label className="text-[10px] font-medium mb-1 block" style={{ color: colors.textMuted }}>
                          Timeout (seconds)
                        </label>
                        <input
                          type="number"
                          value={config.timeout_seconds}
                          onChange={(e) => setConfig(c => ({ ...c, timeout_seconds: parseInt(e.target.value) || 60 }))}
                          className="w-full px-2 py-1.5 rounded text-xs outline-none"
                          style={inputStyle}
                        />
                      </div>

                      {/* RAG Top K */}
                      <div>
                        <label className="text-[10px] font-medium mb-1 block" style={{ color: colors.textMuted }}>
                          RAG Results (top_k)
                        </label>
                        <input
                          type="number"
                          value={config.rag_top_k}
                          onChange={(e) => setConfig(c => ({ ...c, rag_top_k: parseInt(e.target.value) || 5 }))}
                          className="w-full px-2 py-1.5 rounded text-xs outline-none"
                          style={inputStyle}
                        />
                      </div>

                      {/* Auto RAG Toggle */}
                      <div className="flex items-center justify-between">
                        <label className="text-[10px] font-medium" style={{ color: colors.textMuted }}>
                          Auto-use RAG
                        </label>
                        <button
                          onClick={() => setConfig(c => ({ ...c, auto_use_rag: !c.auto_use_rag }))}
                          className="p-0.5"
                        >
                          {config.auto_use_rag ? (
                            <ToggleRight className="w-6 h-6" style={{ color: colors.primary }} />
                          ) : (
                            <ToggleLeft className="w-6 h-6" style={{ color: colors.textMuted }} />
                          )}
                        </button>
                      </div>
                    </div>
                  </motion.div>
                )}
              </AnimatePresence>
            </div>

            {/* Error */}
            {error && (
              <div className="p-3 rounded-lg text-xs" style={{ background: '#ef444420', color: '#ef4444', border: '1px solid #ef444440' }}>
                {error}
              </div>
            )}
          </div>

          {/* Footer */}
          <div className="flex items-center justify-end gap-3 px-6 py-4" style={{ borderTop: `1px solid ${colors.border}` }}>
            <button
              onClick={onClose}
              className="px-4 py-2 rounded-lg text-xs font-medium transition-opacity hover:opacity-70"
              style={{ color: colors.textMuted }}
            >
              Cancel
            </button>
            <button
              onClick={handleSave}
              disabled={saving}
              className="flex items-center gap-2 px-4 py-2 rounded-lg text-xs font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
              style={{ background: colors.primary }}
            >
              {saving ? (
                <motion.div animate={{ rotate: 360 }} transition={{ duration: 1, repeat: Infinity, ease: 'linear' }}>
                  <Zap className="w-3.5 h-3.5" />
                </motion.div>
              ) : (
                <Save className="w-3.5 h-3.5" />
              )}
              {editAgent ? 'Update Agent' : 'Create Agent'}
            </button>
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  );
};

export default AgentBuilder;
