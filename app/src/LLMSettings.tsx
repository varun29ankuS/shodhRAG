import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTheme } from './contexts/ThemeContext';
import { notify } from './lib/notify';
import { migrateLegacyApiKeys } from './lib/apiKeyMigration';
import {
  Brain, Cloud, Power, Settings, CheckCircle, AlertCircle,
  FileCode, X, FolderOpen, RefreshCw, Play, FlaskConical,
  ChevronDown, ChevronUp,
} from 'lucide-react';

interface LLMInfo {
  provider: string;
  model: string;
  context_window: number;
  supports_streaming: boolean;
  is_local: boolean;
  memory_usage?: {
    ram_mb: number;
    vram_mb?: number;
    model_size_mb: number;
  };
  /** `local`, `external` or `disabled`. */
  mode: string;
}

/** GGUF models that run well with llama.cpp on a typical laptop (suggestions only). */
const SUGGESTED_GGUF_MODELS = ['Qwen3 4B', 'Qwen3 8B', 'Gemma 3 4B', 'Llama 3.2 3B'] as const;

interface LLMSettingsProps {
  onClose: () => void;
  onStatusChange?: () => void;
  /** Render inline inside a page (no modal overlay, header or close button). */
  embedded?: boolean;
}

type Provider = 'openai' | 'anthropic' | 'openrouter' | 'kimi' | 'grok' | 'perplexity' | 'google' | 'baseten';

const EMPTY_KEY_DRAFTS: Record<Provider, string> = {
  openai: '', anthropic: '', openrouter: '', kimi: '', grok: '', perplexity: '', google: '', baseten: '',
};

const PROVIDERS: { id: Provider; label: string; defaultModel: string; keyPlaceholder: string; helpUrl: string; models: { value: string; label: string }[] }[] = [
  {
    id: 'openai', label: 'OpenAI', defaultModel: 'gpt-4o-mini', keyPlaceholder: 'sk-...', helpUrl: 'platform.openai.com',
    models: [
      { value: 'gpt-4o-mini', label: 'GPT-4o Mini' },
      { value: 'gpt-4o', label: 'GPT-4o' },
      { value: 'gpt-4-turbo', label: 'GPT-4 Turbo' },
      { value: 'gpt-4', label: 'GPT-4' },
    ],
  },
  {
    id: 'anthropic', label: 'Anthropic', defaultModel: 'claude-3-haiku-20240307', keyPlaceholder: 'sk-ant-...', helpUrl: 'console.anthropic.com',
    models: [
      { value: 'claude-3-haiku-20240307', label: 'Claude 3 Haiku' },
      { value: 'claude-3-sonnet-20240229', label: 'Claude 3 Sonnet' },
      { value: 'claude-3-opus-20240229', label: 'Claude 3 Opus' },
    ],
  },
  {
    id: 'openrouter', label: 'OpenRouter', defaultModel: 'anthropic/claude-haiku-4.5', keyPlaceholder: 'sk-or-...', helpUrl: 'openrouter.ai/keys',
    models: [
      { value: 'anthropic/claude-haiku-4.5', label: 'Claude Haiku 4.5' },
      { value: 'openai/gpt-5-mini', label: 'GPT-5 mini' },
      { value: 'qwen/qwen3-30b-a3b-instruct-2507', label: 'Qwen3 30B A3B Instruct' },
      { value: 'deepseek/deepseek-chat', label: 'DeepSeek Chat' },
      { value: 'stealth/space-bunny-alpha', label: 'Space Bunny Alpha (stealth: prompts may be logged by the provider)' },
    ],
  },
  {
    id: 'kimi', label: 'Kimi', defaultModel: 'moonshot-v1-8k', keyPlaceholder: 'sk-...', helpUrl: 'platform.moonshot.cn',
    models: [
      { value: 'moonshot-v1-8k', label: 'Moonshot v1 8K' },
      { value: 'moonshot-v1-32k', label: 'Moonshot v1 32K' },
      { value: 'moonshot-v1-128k', label: 'Moonshot v1 128K' },
    ],
  },
  {
    id: 'grok', label: 'Grok', defaultModel: 'grok-2-1212', keyPlaceholder: 'xai-...', helpUrl: 'x.ai/api',
    models: [
      { value: 'grok-2-1212', label: 'Grok 2 (Latest)' },
      { value: 'grok-2-vision-1212', label: 'Grok 2 Vision' },
      { value: 'grok-beta', label: 'Grok Beta' },
    ],
  },
  {
    id: 'perplexity', label: 'Perplexity', defaultModel: 'llama-3.1-sonar-small-128k-online', keyPlaceholder: 'pplx-...', helpUrl: 'perplexity.ai',
    models: [
      { value: 'llama-3.1-sonar-large-128k-online', label: 'Sonar Large Online' },
      { value: 'llama-3.1-sonar-small-128k-online', label: 'Sonar Small Online' },
      { value: 'llama-3.1-8b-instruct', label: 'Llama 3.1 8B' },
    ],
  },
  {
    id: 'google', label: 'Google Gemini', defaultModel: 'gemini-2.0-flash-exp', keyPlaceholder: 'AIza...', helpUrl: 'console.cloud.google.com/apis/credentials',
    models: [
      { value: 'gemini-2.5-pro-exp-03-25', label: 'Gemini 2.5 Pro (Latest)' },
      { value: 'gemini-2.0-flash-exp', label: 'Gemini 2.0 Flash' },
      { value: 'gemini-1.5-pro', label: 'Gemini 1.5 Pro' },
      { value: 'gemini-1.5-flash', label: 'Gemini 1.5 Flash' },
      { value: 'gemini-pro', label: 'Gemini Pro' },
    ],
  },
  {
    id: 'baseten', label: 'Baseten', defaultModel: 'deepseek-ai/DeepSeek-V3-0324', keyPlaceholder: 'bt-...', helpUrl: 'baseten.co',
    models: [
      { value: 'deepseek-ai/DeepSeek-V3-0324', label: 'DeepSeek V3' },
    ],
  },
];

export default function LLMSettings({ onClose, onStatusChange, embedded = false }: LLMSettingsProps) {
  const { colors } = useTheme();
  const [llmMode, setLlmMode] = useState<'local' | 'external' | 'disabled'>('disabled');
  const [selectedProvider, setSelectedProvider] = useState<Provider>('openai');
  // Keys typed in this session that are not saved yet. Saved keys live only in
  // the OS credential store (backend); the UI only learns which providers have one.
  const [keyDrafts, setKeyDrafts] = useState<Record<Provider, string>>(EMPTY_KEY_DRAFTS);
  const [configuredProviders, setConfiguredProviders] = useState<Provider[]>([]);
  const [externalModel, setExternalModel] = useState(
    () => PROVIDERS.find(p => p.id === 'openai')?.defaultModel ?? ''
  );
  const [llmInfo, setLlmInfo] = useState<LLMInfo | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [customModelPath, setCustomModelPath] = useState<string | null>(null);
  const [temperature, setTemperature] = useState(0.7);
  const [maxTokens, setMaxTokens] = useState(1024);
  const [topP, setTopP] = useState(0.95);
  const [topK, setTopK] = useState(40);
  const [expandedSections, setExpandedSections] = useState({ local: true, api: true, params: false, status: true });

  const providerConfig = PROVIDERS.find(p => p.id === selectedProvider)!;
  const draftKey = keyDrafts[selectedProvider].trim();
  const keySaved = configuredProviders.includes(selectedProvider);
  const isModelReady = !!customModelPath;
  const localActive = llmInfo?.mode === 'local';

  useEffect(() => {
    loadSettings();
  }, []);

  const refreshConfiguredProviders = async () => {
    const providers = await invoke<string[]>('get_configured_providers');
    setConfiguredProviders(
      providers.filter((p): p is Provider => PROVIDERS.some(known => known.id === p))
    );
  };

  const loadSettings = async () => {
    try {
      try {
        const info = await invoke<LLMInfo>('get_llm_info');
        setLlmInfo(info);
        if (info.mode === 'local' || info.mode === 'external' || info.mode === 'disabled') {
          setLlmMode(info.mode);
        }
      } catch {
        setLlmInfo(null);
      }
      const customPath = await invoke<string | null>('get_custom_model_path');
      setCustomModelPath(customPath);
      await migrateLegacyApiKeys();
      await refreshConfiguredProviders();
    } catch (error) {
      console.error('Failed to load LLM settings:', error);
    }
  };

  const handleBrowseModel = async () => {
    try {
      const modelPath = await invoke<string>('browse_model_file');
      if (modelPath) {
        await invoke<string>('set_custom_model_path', { modelPath });
        setCustomModelPath(modelPath);
        notify.success('Model file selected');
      }
    } catch (error) {
      if (error !== 'No file selected') {
        notify.error(`Failed to select model: ${error}`);
      }
    }
  };

  const handleActivateLocal = async () => {
    setIsLoading(true);
    try {
      await invoke('switch_llm_mode', { mode: 'local' });
      await loadSettings();
      onStatusChange?.();
      notify.success('Local model activated (llama.cpp)');
    } catch (error) {
      notify.error(`Failed to activate model: ${error}`);
    } finally {
      setIsLoading(false);
    }
  };

  const handleModeChange = async (mode: 'local' | 'external' | 'disabled') => {
    if (mode === 'local') {
      setLlmMode(mode);
      if (isModelReady) await handleActivateLocal();
      return;
    }

    setIsLoading(true);
    try {
      if (mode === 'external') {
        if (draftKey) {
          await saveApiKey(selectedProvider, draftKey);
        } else if (!keySaved) {
          notify.warning(`Enter your ${providerConfig.label} API key first`, {
            description: `Get one at ${providerConfig.helpUrl}`,
          });
          setIsLoading(false);
          return;
        }
        await invoke('switch_llm_mode', { mode: 'external', model: externalModel, provider: selectedProvider });
      } else {
        await invoke('switch_llm_mode', { mode: 'disabled' });
      }
      setLlmMode(mode);
      await loadSettings();
      onStatusChange?.();
      notify.success(mode === 'external' ? `Connected to ${providerConfig.label}` : 'LLM disabled');
    } catch (error) {
      notify.error(`Failed to change mode: ${error}`);
    } finally {
      setIsLoading(false);
    }
  };

  const handleApiKeyChange = (provider: Provider, key: string) => {
    setKeyDrafts(prev => ({ ...prev, [provider]: key }));
  };

  /** Store a key in the OS credential store and drop it from component state. */
  const saveApiKey = async (provider: Provider, key: string) => {
    await invoke('set_api_key', { provider, apiKey: key });
    setKeyDrafts(prev => ({ ...prev, [provider]: '' }));
    await refreshConfiguredProviders();
  };

  const handleSaveApiKey = async () => {
    if (!draftKey) return;
    try {
      await saveApiKey(selectedProvider, draftKey);
      notify.success(`${providerConfig.label} API key saved to the system keychain`);
    } catch (error) {
      notify.error(`Failed to save API key: ${error}`);
    }
  };

  const handleRemoveApiKey = async () => {
    try {
      await invoke('delete_api_key', { provider: selectedProvider });
      setKeyDrafts(prev => ({ ...prev, [selectedProvider]: '' }));
      await refreshConfiguredProviders();
      notify.success(`${providerConfig.label} API key removed`);
    } catch (error) {
      notify.error(`Failed to remove API key: ${error}`);
    }
  };

  const handleConfigUpdate = async () => {
    try {
      setIsLoading(true);
      await invoke('update_llm_config', { temperature, maxTokens, topP, topK });
      notify.success('Configuration updated');
    } catch (error) {
      notify.error(`Failed to update config: ${error}`);
    } finally {
      setIsLoading(false);
    }
  };

  const handleTestInference = async () => {
    setIsLoading(true);
    try {
      const response = await invoke<string>('test_llm_inference', {
        prompt: 'Hello! Can you confirm you are working? Please respond with a brief message.',
      });
      notify.success('Model is working', { description: String(response).slice(0, 120) });
    } catch (error) {
      notify.error(`Inference test failed: ${error}`);
    } finally {
      setIsLoading(false);
    }
  };

  const toggleSection = (key: keyof typeof expandedSections) => {
    setExpandedSections(prev => ({ ...prev, [key]: !prev[key] }));
  };

  // --- Shared style fragments ---
  const sectionStyle: React.CSSProperties = {
    padding: '20px 24px',
    borderBottom: `1px solid ${colors.border}`,
  };

  const sectionHeaderStyle: React.CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    cursor: 'pointer',
    userSelect: 'none',
  };

  const selectStyle: React.CSSProperties = {
    width: '100%',
    padding: '10px',
    borderRadius: '6px',
    border: `1px solid ${colors.border}`,
    background: colors.inputBg,
    color: colors.text,
    fontSize: '13px',
    cursor: 'pointer',
    outline: 'none',
  };

  const btnPrimary: React.CSSProperties = {
    padding: '10px 16px',
    fontSize: '13px',
    fontWeight: 600,
    backgroundColor: colors.primary,
    color: colors.primaryText,
    border: 'none',
    borderRadius: '6px',
    cursor: isLoading ? 'not-allowed' : 'pointer',
    opacity: isLoading ? 0.6 : 1,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: '6px',
    transition: 'opacity 0.15s',
  };

  const btnSecondary: React.CSSProperties = {
    padding: '10px 16px',
    fontSize: '13px',
    fontWeight: 600,
    backgroundColor: colors.buttonBg,
    color: colors.buttonText,
    border: `1px solid ${colors.border}`,
    borderRadius: '6px',
    cursor: 'pointer',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: '6px',
    transition: 'background 0.15s',
  };

  return (
    <div style={embedded ? undefined : {
      position: 'fixed',
      top: 0, left: 0, right: 0, bottom: 0,
      background: 'rgba(0, 0, 0, 0.6)',
      backdropFilter: 'blur(4px)',
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      zIndex: 10000,
      animation: 'fadeIn 0.2s ease',
    }}>
      <div style={embedded ? undefined : {
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: '12px',
        width: '90%',
        maxWidth: '680px',
        maxHeight: '90vh',
        overflowY: 'auto',
        boxShadow: '0 16px 48px rgba(0,0,0,0.25)',
      }}>
        {/* Header (modal mode only) */}
        {!embedded && <div style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          padding: '20px 24px',
          borderBottom: `1px solid ${colors.border}`,
        }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
            <Brain size={20} color={colors.primary} />
            <h2 style={{ color: colors.text, margin: 0, fontSize: '17px', fontWeight: 600 }}>AI Configuration</h2>
          </div>
          <button
            onClick={onClose}
            style={{
              width: '32px', height: '32px',
              background: 'transparent',
              border: 'none',
              color: colors.textMuted,
              cursor: 'pointer',
              borderRadius: '6px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
            onMouseEnter={e => { e.currentTarget.style.background = colors.bgHover; e.currentTarget.style.color = colors.text; }}
            onMouseLeave={e => { e.currentTarget.style.background = 'transparent'; e.currentTarget.style.color = colors.textMuted; }}
          >
            <X size={18} />
          </button>
        </div>}

        {/* Mode Selection */}
        <div style={sectionStyle}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '14px' }}>
            <span style={{ color: colors.textTertiary, fontSize: '12px', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' }}>
              Deployment Mode
            </span>
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: '8px' }}>
            {([
              { mode: 'local' as const, icon: Brain, label: 'Local', desc: 'Run on device' },
              { mode: 'external' as const, icon: Cloud, label: 'Cloud API', desc: 'External providers' },
              { mode: 'disabled' as const, icon: Power, label: 'Disabled', desc: 'Search only' },
            ]).map(({ mode, icon: Icon, label, desc }) => {
              const active = llmMode === mode;
              return (
                <button
                  key={mode}
                  onClick={() => handleModeChange(mode)}
                  disabled={isLoading}
                  style={{
                    padding: '14px 12px',
                    background: active ? `${colors.primary}12` : colors.bgSecondary,
                    border: `1px solid ${active ? colors.primary : colors.border}`,
                    borderRadius: '8px',
                    cursor: isLoading ? 'not-allowed' : 'pointer',
                    display: 'flex',
                    flexDirection: 'column',
                    alignItems: 'center',
                    gap: '6px',
                    transition: 'all 0.15s',
                    opacity: isLoading ? 0.6 : 1,
                  }}
                >
                  <Icon size={22} color={active ? colors.primary : colors.textMuted} />
                  <span style={{ fontSize: '13px', fontWeight: 600, color: active ? colors.primary : colors.text }}>{label}</span>
                  <span style={{ fontSize: '11px', color: colors.textMuted }}>{desc}</span>
                </button>
              );
            })}
          </div>
        </div>

        {/* Local Model Setup */}
        {llmMode === 'local' && (
          <div style={sectionStyle}>
            <div
              style={sectionHeaderStyle}
              onClick={() => toggleSection('local')}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                <FileCode size={16} color={colors.primary} />
                <span style={{ color: colors.text, fontSize: '14px', fontWeight: 600 }}>Local Model Setup</span>
              </div>
              {expandedSections.local ? <ChevronUp size={16} color={colors.textMuted} /> : <ChevronDown size={16} color={colors.textMuted} />}
            </div>

            {expandedSections.local && (
              <div style={{ marginTop: '16px' }}>
                <p style={{ color: colors.textMuted, fontSize: '12px', margin: '0 0 16px', lineHeight: 1.5 }}>
                  Runs a GGUF model on this computer with llama.cpp (CPU). Nothing leaves your device.
                </p>

                {/* Model config card */}
                <div style={{
                  border: `1px solid ${isModelReady ? colors.success : colors.warning}`,
                  borderRadius: '8px',
                  padding: '16px',
                  background: colors.bgSecondary,
                }}>
                  {/* Status */}
                  <div style={{ display: 'flex', alignItems: 'center', gap: '10px', marginBottom: '14px' }}>
                    <div style={{
                      width: '32px', height: '32px', borderRadius: '50%',
                      backgroundColor: isModelReady ? colors.success : colors.warning,
                      display: 'flex', alignItems: 'center', justifyContent: 'center',
                    }}>
                      {isModelReady ? <CheckCircle size={18} color="#fff" /> : <AlertCircle size={18} color="#fff" />}
                    </div>
                    <div>
                      <div style={{ fontSize: '14px', fontWeight: 600, color: colors.text }}>
                        {isModelReady ? 'Model Ready' : 'Configuration Required'}
                      </div>
                      <div style={{ fontSize: '12px', color: colors.textMuted, marginTop: '2px' }}>
                        {isModelReady
                          ? 'Your model is configured and ready to use'
                          : 'Select a .gguf model file'}
                      </div>
                    </div>
                  </div>

                  {/* File paths */}
                  <div style={{
                    padding: '10px', background: colors.bgTertiary, borderRadius: '6px',
                    border: `1px solid ${colors.border}`, marginBottom: '14px',
                    display: 'flex', flexDirection: 'column', gap: '8px',
                  }}>
                    <FileRow label="GGUF Model" path={customModelPath} colors={colors} />
                  </div>

                  {/* Action buttons */}
                  <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
                    <button onClick={handleBrowseModel} style={{ ...btnSecondary, flex: 1 }}>
                      {customModelPath ? <><RefreshCw size={14} /> Change Model</> : <><FolderOpen size={14} /> Select Model</>}
                    </button>
                  </div>

                  {isModelReady && (
                    <div style={{ display: 'flex', gap: '8px', marginTop: '10px' }}>
                      {!localActive ? (
                        <button onClick={handleActivateLocal} disabled={isLoading} style={{ ...btnPrimary, flex: 1, backgroundColor: colors.success }}>
                          <Play size={14} /> Activate Model
                        </button>
                      ) : (
                        <div style={{
                          flex: 1, padding: '10px', backgroundColor: colors.success, color: '#fff',
                          borderRadius: '6px', textAlign: 'center', fontWeight: 600, fontSize: '13px',
                          display: 'flex', alignItems: 'center', justifyContent: 'center', gap: '6px',
                        }}>
                          <CheckCircle size={14} /> Model Active
                        </div>
                      )}
                      {localActive && (
                        <button onClick={handleTestInference} disabled={isLoading} style={{ ...btnSecondary }}>
                          <FlaskConical size={14} /> Test
                        </button>
                      )}
                    </div>
                  )}

                  {!customModelPath && (
                    <div style={{
                      marginTop: '12px', padding: '10px', background: colors.bgTertiary,
                      border: `1px solid ${colors.border}`, borderRadius: '6px',
                      fontSize: '12px', color: colors.textMuted, lineHeight: 1.5,
                      display: 'flex', alignItems: 'flex-start', gap: '8px',
                    }}>
                      <Settings size={13} color={colors.primary} style={{ marginTop: '2px', flexShrink: 0 }} />
                      <span>
                        <strong style={{ color: colors.text }}>Quick Start:</strong>{' '}
                        Download a GGUF model (Q4_K_M is a good balance of size and quality) and select the .gguf file.
                        Suggested: {SUGGESTED_GGUF_MODELS.join(', ')}. Shodh does not download models for you.
                      </span>
                    </div>
                  )}
                </div>

              </div>
            )}
          </div>
        )}

        {/* External API Configuration */}
        <div style={sectionStyle}>
          <div
            style={sectionHeaderStyle}
            onClick={() => toggleSection('api')}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
              <Cloud size={16} color={colors.primary} />
              <span style={{ color: colors.text, fontSize: '14px', fontWeight: 600 }}>API Provider</span>
            </div>
            {expandedSections.api ? <ChevronUp size={16} color={colors.textMuted} /> : <ChevronDown size={16} color={colors.textMuted} />}
          </div>

          {expandedSections.api && (
            <div style={{ marginTop: '16px' }}>
              {/* Provider tabs */}
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: '6px', marginBottom: '16px' }}>
                {PROVIDERS.map(provider => {
                  const active = selectedProvider === provider.id;
                  return (
                    <button
                      key={provider.id}
                      onClick={() => {
                        setSelectedProvider(provider.id);
                        setExternalModel(provider.defaultModel);
                      }}
                      style={{
                        padding: '6px 12px',
                        borderRadius: '6px',
                        border: `1px solid ${active ? colors.primary : colors.border}`,
                        background: active ? `${colors.primary}12` : colors.bgSecondary,
                        color: active ? colors.primary : colors.text,
                        fontSize: '12px',
                        fontWeight: active ? 600 : 500,
                        cursor: 'pointer',
                        transition: 'all 0.15s',
                      }}
                    >
                      {provider.label}
                    </button>
                  );
                })}
              </div>

              {/* API Key + Model config */}
              <div style={{
                padding: '14px', background: colors.bgSecondary, borderRadius: '8px',
                border: `1px solid ${colors.border}`, display: 'flex', flexDirection: 'column', gap: '12px',
              }}>
                {/* API Key */}
                <div>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '6px' }}>
                    <label style={{ color: colors.text, fontWeight: 600, fontSize: '12px' }}>API Key</label>
                    {keySaved ? (
                      <span style={{ display: 'flex', alignItems: 'center', gap: '3px', color: colors.success, fontSize: '11px', fontWeight: 500 }}>
                        <CheckCircle size={12} /> Key configured
                      </span>
                    ) : draftKey ? (
                      <span style={{ display: 'flex', alignItems: 'center', gap: '3px', color: colors.textMuted, fontSize: '11px', fontWeight: 500 }}>
                        <AlertCircle size={12} /> Not saved yet
                      </span>
                    ) : (
                      <span style={{ display: 'flex', alignItems: 'center', gap: '3px', color: colors.error, fontSize: '11px', fontWeight: 500 }}>
                        <AlertCircle size={12} /> Required
                      </span>
                    )}
                  </div>
                  <div style={{ display: 'flex', gap: '6px' }}>
                    <input
                      type="password"
                      value={keyDrafts[selectedProvider]}
                      onChange={e => handleApiKeyChange(selectedProvider, e.target.value)}
                      placeholder={keySaved ? 'Key configured (type to replace)' : providerConfig.keyPlaceholder}
                      autoComplete="off"
                      spellCheck={false}
                      style={{
                        flex: 1,
                        minWidth: 0,
                        padding: '9px 10px',
                        borderRadius: '6px',
                        border: `1px solid ${keySaved || draftKey ? colors.success : colors.border}`,
                        background: colors.inputBg,
                        color: colors.text,
                        fontSize: '13px',
                        fontFamily: 'monospace',
                        outline: 'none',
                        boxSizing: 'border-box',
                      }}
                    />
                    <button
                      type="button"
                      onClick={handleSaveApiKey}
                      disabled={!draftKey}
                      style={{
                        padding: '6px 12px',
                        borderRadius: '6px',
                        border: `1px solid ${colors.border}`,
                        background: colors.bgSecondary,
                        color: draftKey ? colors.text : colors.textMuted,
                        fontSize: '12px',
                        fontWeight: 500,
                        cursor: draftKey ? 'pointer' : 'not-allowed',
                      }}
                    >
                      Save
                    </button>
                    {keySaved && (
                      <button
                        type="button"
                        onClick={handleRemoveApiKey}
                        style={{
                          padding: '6px 12px',
                          borderRadius: '6px',
                          border: `1px solid ${colors.border}`,
                          background: colors.bgSecondary,
                          color: colors.error,
                          fontSize: '12px',
                          fontWeight: 500,
                          cursor: 'pointer',
                        }}
                      >
                        Remove
                      </button>
                    )}
                  </div>
                  <small style={{ color: colors.textMuted, fontSize: '11px', display: 'block', marginTop: '4px' }}>
                    Get your API key from {providerConfig.helpUrl}. Saved keys go to the system keychain, not app storage.
                  </small>
                </div>

                {/* Model select */}
                <div>
                  <label htmlFor="external-model-id" style={{ color: colors.text, fontWeight: 600, fontSize: '12px', display: 'block', marginBottom: '6px' }}>Model</label>
                  <input
                    id="external-model-id"
                    list={`models-${providerConfig.id}`}
                    value={externalModel}
                    onChange={e => setExternalModel(e.target.value.trim())}
                    placeholder={providerConfig.defaultModel}
                    spellCheck={false}
                    autoComplete="off"
                    style={{ ...selectStyle, fontFamily: 'monospace' }}
                  />
                  <datalist id={`models-${providerConfig.id}`}>
                    {providerConfig.models.map(m => (
                      <option key={m.value} value={m.value}>{m.label}</option>
                    ))}
                  </datalist>
                  <small style={{ color: colors.textMuted, fontSize: '11px', display: 'block', marginTop: '4px' }}>
                    Pick a suggestion or type any model ID the provider supports.
                  </small>
                </div>
              </div>
            </div>
          )}
        </div>

        {/* Generation Parameters */}
        <div style={sectionStyle}>
          <div
            style={sectionHeaderStyle}
            onClick={() => toggleSection('params')}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
              <Settings size={16} color={colors.primary} />
              <span style={{ color: colors.text, fontSize: '14px', fontWeight: 600 }}>Parameters</span>
            </div>
            {expandedSections.params ? <ChevronUp size={16} color={colors.textMuted} /> : <ChevronDown size={16} color={colors.textMuted} />}
          </div>

          {expandedSections.params && (
            <div style={{ marginTop: '16px' }}>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '12px', marginBottom: '14px' }}>
                <ParamSlider label="Temperature" value={temperature} min={0} max={2} step={0.1} onChange={setTemperature} hint="Higher = more creative" colors={colors} />
                <ParamNumber label="Max Tokens" value={maxTokens} min={1} max={4096} onChange={setMaxTokens} hint="Max response length" colors={colors} />
                <ParamSlider label="Top P" value={topP} min={0} max={1} step={0.05} onChange={setTopP} hint="Nucleus sampling threshold" decimals={2} colors={colors} />
                <ParamNumber label="Top K" value={topK} min={1} max={100} onChange={setTopK} hint="Top K most likely tokens" colors={colors} />
              </div>
              <button onClick={handleConfigUpdate} disabled={isLoading} style={{ ...btnPrimary, width: '100%' }}>
                Update Configuration
              </button>
            </div>
          )}
        </div>

        {/* Current Status */}
        {llmInfo && (
          <div style={{ ...sectionStyle, borderBottom: 'none' }}>
            <div
              style={sectionHeaderStyle}
              onClick={() => toggleSection('status')}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                <CheckCircle size={16} color={colors.success} />
                <span style={{ color: colors.text, fontSize: '14px', fontWeight: 600 }}>Status</span>
              </div>
              {expandedSections.status ? <ChevronUp size={16} color={colors.textMuted} /> : <ChevronDown size={16} color={colors.textMuted} />}
            </div>

            {expandedSections.status && (
              <div style={{
                marginTop: '12px', padding: '12px', background: colors.bgSecondary,
                borderRadius: '6px', border: `1px solid ${colors.border}`,
              }}>
                <StatusRow label="Provider" value={llmInfo.provider} colors={colors} />
                <StatusRow label="Model" value={llmInfo.model} mono colors={colors} />
                <StatusRow label="Context" value={`${llmInfo.context_window.toLocaleString()} tokens`} colors={colors} />
                {llmInfo.memory_usage && (
                  <StatusRow label="Memory" value={`${llmInfo.memory_usage.ram_mb.toLocaleString()} MB`} colors={colors} last />
                )}
              </div>
            )}
          </div>
        )}
      </div>

      <style>{`
        @keyframes fadeIn { from { opacity: 0 } to { opacity: 1 } }
      `}</style>
    </div>
  );
}

// --- Sub-components ---

function FileRow({ label, path, fallback, colors }: { label: string; path: string | null; fallback?: string; colors: Record<string, string> }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
      {path ? <CheckCircle size={14} color={colors.success} /> : (
        <div style={{ width: 14, height: 14, borderRadius: '50%', border: `1.5px solid ${colors.textMuted}`, opacity: 0.4 }} />
      )}
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: '11px', fontWeight: 600, color: colors.text }}>{label}</div>
        {path ? (
          <div style={{ fontSize: '11px', color: colors.textMuted, fontFamily: 'monospace', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
            {path.split('\\').pop()}
          </div>
        ) : (
          <div style={{ fontSize: '11px', color: colors.textMuted, fontStyle: 'italic', opacity: 0.7 }}>
            {fallback || 'Not selected'}
          </div>
        )}
      </div>
    </div>
  );
}

function ParamSlider({ label, value, min, max, step, onChange, hint, decimals = 1, colors }: {
  label: string; value: number; min: number; max: number; step: number; onChange: (v: number) => void; hint: string; decimals?: number; colors: Record<string, string>;
}) {
  return (
    <div style={{ padding: '12px', background: colors.bgSecondary, borderRadius: '6px', border: `1px solid ${colors.border}` }}>
      <label style={{ color: colors.text, fontWeight: 600, fontSize: '12px', display: 'block', marginBottom: '8px' }}>{label}</label>
      <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
        <input
          type="range" min={min} max={max} step={step} value={value}
          onChange={e => onChange(parseFloat(e.target.value))}
          style={{ flex: 1, accentColor: colors.primary }}
        />
        <span style={{ color: colors.text, fontSize: '13px', fontWeight: 600, minWidth: '36px', textAlign: 'right' }}>
          {value.toFixed(decimals)}
        </span>
      </div>
      <small style={{ color: colors.textMuted, fontSize: '11px', display: 'block', marginTop: '4px' }}>{hint}</small>
    </div>
  );
}

function ParamNumber({ label, value, min, max, onChange, hint, colors }: {
  label: string; value: number; min: number; max: number; onChange: (v: number) => void; hint: string; colors: Record<string, string>;
}) {
  return (
    <div style={{ padding: '12px', background: colors.bgSecondary, borderRadius: '6px', border: `1px solid ${colors.border}` }}>
      <label style={{ color: colors.text, fontWeight: 600, fontSize: '12px', display: 'block', marginBottom: '8px' }}>{label}</label>
      <input
        type="number" value={value} onChange={e => onChange(parseInt(e.target.value) || 0)} min={min} max={max}
        style={{
          width: '100%', padding: '7px 10px', borderRadius: '6px',
          border: `1px solid ${colors.border}`, background: colors.inputBg,
          color: colors.text, fontSize: '13px', outline: 'none', boxSizing: 'border-box',
        }}
      />
      <small style={{ color: colors.textMuted, fontSize: '11px', display: 'block', marginTop: '4px' }}>{hint}</small>
    </div>
  );
}

function StatusRow({ label, value, mono, last, colors }: { label: string; value: string; mono?: boolean; last?: boolean; colors: Record<string, string> }) {
  return (
    <div style={{
      display: 'flex', justifyContent: 'space-between', alignItems: 'center',
      padding: '8px 0',
      borderBottom: last ? 'none' : `1px solid ${colors.border}`,
    }}>
      <span style={{ color: colors.textMuted, fontSize: '12px', fontWeight: 500 }}>{label}</span>
      <span style={{ color: colors.text, fontSize: '12px', fontWeight: 600, fontFamily: mono ? 'monospace' : 'inherit' }}>{value}</span>
    </div>
  );
}
