import React, { useState, useEffect, useCallback } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Bot, Activity, Clock, CheckCircle2, XCircle, AlertTriangle,
  ChevronDown, ChevronRight, Wrench, Zap, ToggleLeft, ToggleRight,
  RefreshCw, Loader2, Circle, Play, Search, FileText, Code, Brain,
  Sparkles, Wand2, Plus, Trash2, Send, Edit3, MessageSquare, ArrowRight, Users, Crown, GitBranch, ListOrdered,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useTheme } from '../contexts/ThemeContext';
import { AgentBuilder } from './AgentBuilder';
import { CrewBuilder } from './CrewBuilder';

interface AgentInfo {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  execution_count: number;
  avg_execution_time_ms: number;
  capabilities: string[];
}

interface ExecutionLogEntry {
  id: string;
  agent_id: string;
  agent_name: string;
  query: string;
  response: string | null;
  status: string;
  execution_time_ms: number | null;
  steps_count: number;
  tools_used: string[];
  success: boolean;
  error_message: string | null;
  started_at: string;
}

interface AgentHealthEntry {
  agent_id: string;
  status: string;
  health_score: number;
  consecutive_failures: number;
  last_success_at: string | null;
  last_failure_at: string | null;
}

interface ActiveExecution {
  execution_id: string;
  agent_id: string;
  agent_name: string;
  query: string;
  started_at: string;
  elapsed_ms: number;
  current_step: number;
  total_steps: number;
  current_step_type: string;
  current_message: string;
  progress_percentage: number;
}

interface AgentDashboard {
  agents: AgentInfo[];
  total_runs: number;
  successful_runs: number;
  failed_runs: number;
  success_rate: number;
  active_now: number;
  recent_executions: ExecutionLogEntry[];
  health: AgentHealthEntry[];
}

const CAPABILITY_ICONS: Record<string, React.ElementType> = {
  RAGSearch: Search,
  CodeGeneration: Code,
  Analysis: Brain,
  Summarization: FileText,
  Creative: Sparkles,
  ToolUse: Wand2,
  ExternalAPI: Zap,
};

function relativeTimeShort(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const secs = Math.floor(diff / 1000);
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.floor(hrs / 24)}d ago`;
}

interface AgentDefinition {
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  enabled: boolean;
  config: {
    temperature: number;
    max_tokens: number;
    top_p: number;
    stream: boolean;
    max_tool_calls: number;
    timeout_seconds: number;
    auto_use_rag: boolean;
    rag_top_k: number;
  };
  capabilities: string[];
  tools: string[];
  metadata: Record<string, string>;
}

interface ExecutionResult {
  response: string;
  steps: Array<{
    step_number: number;
    step_type: string;
    duration_ms: number;
    input: string;
    output: string;
    tool_used: string | null;
    success: boolean;
  }>;
  tools_used: string[];
  execution_time_ms: number;
  success: boolean;
  error: string | null;
}

// Crew types
interface CrewDefinition {
  id?: string;
  name: string;
  description: string;
  agents: CrewMember[];
  process: string;
  coordinator_id?: string;
  config: { timeout_seconds: number; verbose: boolean };
}

interface CrewMember {
  agent_id: string;
  role: string;
  goal: string;
  order: number;
}

interface CrewExecutionResult {
  success: boolean;
  final_output: string;
  agent_outputs: CrewAgentOutput[];
  execution_time_ms: number;
  error: string | null;
}

interface CrewAgentOutput {
  agent_id: string;
  agent_name: string;
  role: string;
  output: string;
  execution_time_ms: number;
  tools_used: string[];
}

interface AgentsPanelProps {
  onRunAgent?: (agentId: string, query: string) => void;
}

export default function AgentsPanel({ onRunAgent }: AgentsPanelProps = {}) {
  const { colors, theme } = useTheme();
  const [dashboard, setDashboard] = useState<AgentDashboard | null>(null);
  const [activeExecs, setActiveExecs] = useState<ActiveExecution[]>([]);
  const [loading, setLoading] = useState(true);
  const [expandedAgent, setExpandedAgent] = useState<string | null>(null);
  const [expandedLog, setExpandedLog] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [showBuilder, setShowBuilder] = useState(false);
  const [editingAgent, setEditingAgent] = useState<AgentDefinition | null>(null);
  const [runDialogAgent, setRunDialogAgent] = useState<string | null>(null);
  const [runQuery, setRunQuery] = useState('');
  const [runningExecution, setRunningExecution] = useState(false);
  const [executionResult, setExecutionResult] = useState<ExecutionResult | null>(null);

  // Crew state
  const [crews, setCrews] = useState<CrewDefinition[]>([]);
  const [showCrewBuilder, setShowCrewBuilder] = useState(false);
  const [expandedCrew, setExpandedCrew] = useState<string | null>(null);
  const [runCrewDialog, setRunCrewDialog] = useState<string | null>(null);
  const [crewTaskInput, setCrewTaskInput] = useState('');
  const [runningCrew, setRunningCrew] = useState(false);
  const [crewResult, setCrewResult] = useState<CrewExecutionResult | null>(null);

  const fetchData = useCallback(async () => {
    try {
      const [dash, active, crewList] = await Promise.all([
        invoke<AgentDashboard>('get_agent_dashboard'),
        invoke<ActiveExecution[]>('get_active_executions'),
        invoke<CrewDefinition[]>('list_crews').catch(() => [] as CrewDefinition[]),
      ]);
      setDashboard(dash);
      setActiveExecs(active);
      setCrews(crewList);
    } catch (err) {
      console.error('Failed to load agent dashboard:', err);
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, [fetchData]);

  const handleToggleAgent = async (agentId: string, enabled: boolean) => {
    try {
      await invoke('toggle_agent', { agentId, enabled });
      setDashboard(prev => {
        if (!prev) return prev;
        return {
          ...prev,
          agents: prev.agents.map(a =>
            a.id === agentId ? { ...a, enabled } : a
          ),
        };
      });
    } catch (err) {
      console.error('Failed to toggle agent:', err);
    }
  };

  const handleRefresh = () => {
    setRefreshing(true);
    fetchData();
  };

  const handleAgentCreated = (_agent: AgentDefinition) => {
    fetchData();
    setShowBuilder(false);
    setEditingAgent(null);
  };

  const handleEditAgent = async (agentId: string) => {
    try {
      const def = await invoke<AgentDefinition>('get_agent', { agentId });
      setEditingAgent(def);
      setShowBuilder(true);
    } catch (err) {
      console.error('Failed to load agent for editing:', err);
    }
  };

  const handleDeleteAgent = async (agentId: string) => {
    try {
      await invoke('delete_agent', { agentId });
      fetchData();
    } catch (err) {
      console.error('Failed to delete agent:', err);
    }
  };

  const handleRunAgent = async (agentId: string) => {
    if (!runQuery.trim()) return;

    setRunningExecution(true);
    setExecutionResult(null);

    try {
      const result = await invoke<ExecutionResult>('execute_agent', {
        agentId,
        query: runQuery.trim(),
        spaceId: null,
        conversationHistory: null,
      });
      setExecutionResult(result);
    } catch (err: any) {
      setExecutionResult({
        response: '',
        steps: [],
        tools_used: [],
        execution_time_ms: 0,
        success: false,
        error: err?.toString() || 'Execution failed',
      });
    } finally {
      setRunningExecution(false);
    }
  };

  const handleDeleteCrew = async (crewId: string) => {
    try {
      await invoke('delete_crew', { crewId });
      fetchData();
    } catch (err) {
      console.error('Failed to delete crew:', err);
    }
  };

  const handleRunCrew = async (crewId: string) => {
    if (!crewTaskInput.trim()) return;
    setRunningCrew(true);
    setCrewResult(null);
    try {
      const result = await invoke<CrewExecutionResult>('execute_crew', {
        crewId,
        task: crewTaskInput.trim(),
        spaceId: null,
      });
      setCrewResult(result);
    } catch (err: any) {
      setCrewResult({
        success: false,
        final_output: '',
        agent_outputs: [],
        execution_time_ms: 0,
        error: err?.toString() || 'Crew execution failed',
      });
    } finally {
      setRunningCrew(false);
    }
  };

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center">
        <Loader2 className="w-6 h-6 animate-spin" style={{ color: colors.primary }} />
      </div>
    );
  }

  const agents = dashboard?.agents ?? [];
  const recentExecs = dashboard?.recent_executions ?? [];
  const healthMap = new Map((dashboard?.health ?? []).map(h => [h.agent_id, h]));

  return (
    <div className="h-full overflow-y-auto p-6">
      <div className="max-w-5xl mx-auto space-y-6">
        {/* Header */}
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div
              className="w-9 h-9 rounded-lg flex items-center justify-center"
              style={{ backgroundColor: `${colors.primary}15` }}
            >
              <Bot className="w-5 h-5" style={{ color: colors.primary }} />
            </div>
            <div>
              <h2 className="text-lg font-semibold" style={{ color: colors.text }}>
                AI Agents
              </h2>
              <p className="text-xs" style={{ color: colors.textMuted }}>
                Monitor agent activity, health, and execution logs
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={() => setShowCrewBuilder(true)}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-colors"
              style={{ backgroundColor: `${colors.secondary || colors.primary}15`, color: colors.secondary || colors.primary, border: `1px solid ${colors.secondary || colors.primary}` }}
            >
              <Users className="w-3.5 h-3.5" />
              Create Crew
            </button>
            <button
              onClick={() => { setEditingAgent(null); setShowBuilder(true); }}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium text-white transition-colors"
              style={{ backgroundColor: colors.primary }}
            >
              <Plus className="w-3.5 h-3.5" />
              Create Agent
            </button>
            <button
              onClick={handleRefresh}
              disabled={refreshing}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-colors"
              style={{
                backgroundColor: colors.bgTertiary,
                color: colors.textSecondary,
              }}
            >
              <RefreshCw className={`w-3.5 h-3.5 ${refreshing ? 'animate-spin' : ''}`} />
              Refresh
            </button>
          </div>
        </div>

        {/* Summary Cards */}
        <div className="grid grid-cols-5 gap-3">
          {[
            {
              label: 'Agents',
              value: agents.length,
              icon: Bot,
              color: colors.primary,
            },
            {
              label: 'Active Now',
              value: activeExecs.length,
              icon: Play,
              color: '#22c55e',
            },
            {
              label: 'Total Runs',
              value: dashboard?.total_runs ?? 0,
              icon: Activity,
              color: colors.secondary || '#6366f1',
            },
            {
              label: 'Success Rate',
              value: `${((dashboard?.success_rate ?? 0) * 100).toFixed(0)}%`,
              icon: CheckCircle2,
              color: '#10b981',
            },
            {
              label: 'Failed',
              value: dashboard?.failed_runs ?? 0,
              icon: XCircle,
              color: colors.error,
            },
          ].map(card => {
            const Icon = card.icon;
            return (
              <div
                key={card.label}
                className="rounded-xl border p-3"
                style={{ backgroundColor: colors.bgSecondary, borderColor: colors.border }}
              >
                <div className="flex items-center gap-2 mb-1.5">
                  <Icon className="w-3.5 h-3.5" style={{ color: card.color }} />
                  <span className="text-[10px] font-medium" style={{ color: colors.textMuted }}>
                    {card.label}
                  </span>
                </div>
                <div className="text-xl font-bold" style={{ color: colors.text }}>
                  {card.value}
                </div>
              </div>
            );
          })}
        </div>

        {/* Active Executions (live) */}
        <AnimatePresence>
          {activeExecs.length > 0 && (
            <motion.div
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              className="rounded-xl border overflow-hidden"
              style={{ backgroundColor: `${colors.warning}08`, borderColor: `${colors.warning}30` }}
            >
              <div className="px-4 py-3 flex items-center gap-2">
                <Loader2 className="w-4 h-4 animate-spin" style={{ color: colors.warning }} />
                <span className="text-sm font-semibold" style={{ color: colors.text }}>
                  Live Executions
                </span>
                <span
                  className="text-[10px] px-1.5 py-0.5 rounded-full font-medium"
                  style={{ backgroundColor: `${colors.warning}20`, color: colors.warning }}
                >
                  {activeExecs.length}
                </span>
              </div>
              <div className="px-4 pb-3 space-y-2">
                {activeExecs.map(exec => (
                  <div
                    key={exec.execution_id}
                    className="flex items-center gap-3 p-2.5 rounded-lg"
                    style={{ backgroundColor: colors.bgSecondary }}
                  >
                    <div className="relative">
                      <Bot className="w-4 h-4" style={{ color: colors.primary }} />
                      <div
                        className="absolute -bottom-0.5 -right-0.5 w-2 h-2 rounded-full animate-pulse"
                        style={{ backgroundColor: '#22c55e' }}
                      />
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="text-xs font-medium" style={{ color: colors.text }}>
                        {exec.agent_name}
                      </div>
                      <div className="text-[10px] truncate" style={{ color: colors.textMuted }}>
                        {exec.query}
                      </div>
                    </div>
                    <div className="text-right shrink-0">
                      <div className="text-[10px] font-mono" style={{ color: colors.textSecondary }}>
                        Step {exec.current_step}/{exec.total_steps}
                      </div>
                      {exec.current_step_type && (
                        <div className="text-[10px] flex items-center gap-1" style={{ color: colors.primary }}>
                          <Wrench className="w-2.5 h-2.5" />
                          {exec.current_step_type}
                        </div>
                      )}
                      <div className="text-[10px]" style={{ color: colors.textMuted }}>
                        {(exec.elapsed_ms / 1000).toFixed(1)}s
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Registered Agents */}
        <div>
          <h3 className="text-sm font-semibold mb-3 flex items-center gap-2" style={{ color: colors.text }}>
            <Bot className="w-4 h-4" style={{ color: colors.primary }} />
            Registered Agents
            <span className="text-[10px] font-normal" style={{ color: colors.textMuted }}>
              ({agents.length})
            </span>
          </h3>
          {agents.length === 0 ? (
            <div
              className="rounded-xl border p-8 text-center"
              style={{ backgroundColor: colors.bgSecondary, borderColor: colors.border }}
            >
              <Bot className="w-8 h-8 mx-auto mb-2" style={{ color: colors.textMuted }} />
              <p className="text-sm" style={{ color: colors.textSecondary }}>
                No agents active yet
              </p>
              <p className="text-xs mt-1 max-w-md mx-auto" style={{ color: colors.textMuted }}>
                Agents are created dynamically when your queries need specialized handling.
                Ask the system to "create an agent for ..." in chat, or agents will be
                spun up automatically for complex multi-step tasks.
              </p>
            </div>
          ) : (
            <div className="space-y-2">
              {agents.map(agent => {
                const health = healthMap.get(agent.id);
                const isExpanded = expandedAgent === agent.id;

                return (
                  <div
                    key={agent.id}
                    className="rounded-xl border overflow-hidden transition-colors"
                    style={{ backgroundColor: colors.bgSecondary, borderColor: colors.border }}
                  >
                    <button
                      onClick={() => setExpandedAgent(isExpanded ? null : agent.id)}
                      className="w-full text-left px-4 py-3 flex items-center gap-3"
                    >
                      <div
                        className="w-8 h-8 rounded-lg flex items-center justify-center shrink-0"
                        style={{
                          backgroundColor: agent.enabled ? `${colors.primary}15` : `${colors.textMuted}10`,
                        }}
                      >
                        <Bot
                          className="w-4 h-4"
                          style={{ color: agent.enabled ? colors.primary : colors.textMuted }}
                        />
                      </div>
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2">
                          <span
                            className="text-sm font-medium"
                            style={{ color: agent.enabled ? colors.text : colors.textMuted }}
                          >
                            {agent.name}
                          </span>
                          {!agent.enabled && (
                            <span
                              className="text-[9px] px-1.5 py-px rounded-full font-medium"
                              style={{ backgroundColor: `${colors.textMuted}15`, color: colors.textMuted }}
                            >
                              DISABLED
                            </span>
                          )}
                          {health && (
                            <span
                              className="text-[9px] px-1.5 py-px rounded-full font-medium"
                              style={{
                                backgroundColor:
                                  health.status === 'healthy'
                                    ? '#10b98115'
                                    : health.status === 'degraded'
                                    ? `${colors.warning}15`
                                    : `${colors.error}15`,
                                color:
                                  health.status === 'healthy'
                                    ? '#10b981'
                                    : health.status === 'degraded'
                                    ? colors.warning
                                    : colors.error,
                              }}
                            >
                              {health.status.toUpperCase()}
                            </span>
                          )}
                        </div>
                        <div className="text-xs truncate mt-0.5" style={{ color: colors.textMuted }}>
                          {agent.description}
                        </div>
                      </div>
                      <div className="flex items-center gap-4 shrink-0">
                        <div className="text-right">
                          <div className="text-xs font-mono" style={{ color: colors.textSecondary }}>
                            {agent.execution_count} runs
                          </div>
                          {agent.avg_execution_time_ms > 0 && (
                            <div className="text-[10px]" style={{ color: colors.textMuted }}>
                              avg {(agent.avg_execution_time_ms / 1000).toFixed(1)}s
                            </div>
                          )}
                        </div>
                        {isExpanded ? (
                          <ChevronDown className="w-4 h-4" style={{ color: colors.textMuted }} />
                        ) : (
                          <ChevronRight className="w-4 h-4" style={{ color: colors.textMuted }} />
                        )}
                      </div>
                    </button>

                    <AnimatePresence>
                      {isExpanded && (
                        <motion.div
                          initial={{ height: 0, opacity: 0 }}
                          animate={{ height: 'auto', opacity: 1 }}
                          exit={{ height: 0, opacity: 0 }}
                          transition={{ duration: 0.2 }}
                          className="overflow-hidden"
                        >
                          <div
                            className="px-4 pb-4 pt-1 border-t space-y-3"
                            style={{ borderColor: colors.border }}
                          >
                            {/* Capabilities */}
                            {agent.capabilities.length > 0 && (
                              <div>
                                <span className="text-[10px] font-semibold" style={{ color: colors.textMuted }}>
                                  CAPABILITIES
                                </span>
                                <div className="flex flex-wrap gap-1.5 mt-1">
                                  {agent.capabilities.map(cap => {
                                    const CapIcon = CAPABILITY_ICONS[cap] || Zap;
                                    return (
                                      <span
                                        key={cap}
                                        className="text-[10px] px-2 py-0.5 rounded-full flex items-center gap-1"
                                        style={{
                                          backgroundColor: `${colors.primary}10`,
                                          color: colors.primary,
                                        }}
                                      >
                                        <CapIcon className="w-2.5 h-2.5" />
                                        {cap}
                                      </span>
                                    );
                                  })}
                                </div>
                              </div>
                            )}

                            {/* Health details */}
                            {health && (
                              <div>
                                <span className="text-[10px] font-semibold" style={{ color: colors.textMuted }}>
                                  HEALTH
                                </span>
                                <div className="flex items-center gap-4 mt-1">
                                  <div className="flex items-center gap-1.5">
                                    <div className="w-16 h-1.5 rounded-full overflow-hidden" style={{ backgroundColor: colors.bgTertiary }}>
                                      <div
                                        className="h-full rounded-full transition-all"
                                        style={{
                                          width: `${health.health_score * 100}%`,
                                          backgroundColor:
                                            health.health_score > 0.8 ? '#10b981' :
                                            health.health_score > 0.5 ? colors.warning : colors.error,
                                        }}
                                      />
                                    </div>
                                    <span className="text-[10px] font-mono" style={{ color: colors.textSecondary }}>
                                      {(health.health_score * 100).toFixed(0)}%
                                    </span>
                                  </div>
                                  {health.consecutive_failures > 0 && (
                                    <span className="text-[10px] flex items-center gap-1" style={{ color: colors.error }}>
                                      <AlertTriangle className="w-3 h-3" />
                                      {health.consecutive_failures} consecutive failures
                                    </span>
                                  )}
                                </div>
                              </div>
                            )}

                            {/* Actions row */}
                            <div className="flex items-center justify-between pt-1">
                              <div className="flex items-center gap-2">
                                <button
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    setRunDialogAgent(agent.id);
                                    setRunQuery('');
                                    setExecutionResult(null);
                                  }}
                                  className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] font-medium text-white transition-opacity hover:opacity-90"
                                  style={{ backgroundColor: '#22c55e' }}
                                >
                                  <Play className="w-3 h-3" />
                                  Run
                                </button>
                                <button
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    handleEditAgent(agent.id);
                                  }}
                                  className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] font-medium transition-opacity hover:opacity-90"
                                  style={{ backgroundColor: `${colors.primary}15`, color: colors.primary }}
                                >
                                  <Edit3 className="w-3 h-3" />
                                  Edit
                                </button>
                                <button
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    handleDeleteAgent(agent.id);
                                  }}
                                  className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] font-medium transition-opacity hover:opacity-90"
                                  style={{ backgroundColor: `${colors.error}12`, color: colors.error }}
                                >
                                  <Trash2 className="w-3 h-3" />
                                  Delete
                                </button>
                              </div>
                              <button
                                onClick={(e) => {
                                  e.stopPropagation();
                                  handleToggleAgent(agent.id, !agent.enabled);
                                }}
                                className="flex items-center gap-1.5 text-xs"
                                style={{ color: agent.enabled ? '#10b981' : colors.textMuted }}
                              >
                                {agent.enabled ? (
                                  <ToggleRight className="w-5 h-5" />
                                ) : (
                                  <ToggleLeft className="w-5 h-5" />
                                )}
                                {agent.enabled ? 'Enabled' : 'Disabled'}
                              </button>
                            </div>
                          </div>
                        </motion.div>
                      )}
                    </AnimatePresence>
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* Crews Section */}
        {crews.length > 0 && (
          <div>
            <h3 className="text-sm font-semibold mb-3 flex items-center gap-2" style={{ color: colors.text }}>
              <Users className="w-4 h-4" style={{ color: colors.secondary || '#6366f1' }} />
              Crews ({crews.length})
            </h3>
            <div className="space-y-2">
              {crews.map(crew => {
                const isExpanded = expandedCrew === crew.id;
                return (
                  <div
                    key={crew.id}
                    className="rounded-xl overflow-hidden"
                    style={{ backgroundColor: colors.bgSecondary, border: `1px solid ${colors.border}` }}
                  >
                    <div
                      className="flex items-center justify-between px-4 py-3 cursor-pointer hover:opacity-90"
                      onClick={() => setExpandedCrew(isExpanded ? null : (crew.id || null))}
                    >
                      <div className="flex items-center gap-3">
                        <div className="w-8 h-8 rounded-lg flex items-center justify-center" style={{ backgroundColor: `${colors.secondary || '#6366f1'}15` }}>
                          <Users className="w-4 h-4" style={{ color: colors.secondary || '#6366f1' }} />
                        </div>
                        <div>
                          <div className="text-sm font-medium" style={{ color: colors.text }}>{crew.name}</div>
                          <div className="text-[10px] flex items-center gap-2" style={{ color: colors.textMuted }}>
                            <span>{crew.agents.length} agents</span>
                            <span>|</span>
                            <span className="flex items-center gap-1">
                              {crew.process === 'hierarchical' ? <GitBranch className="w-3 h-3" /> : <ListOrdered className="w-3 h-3" />}
                              {crew.process}
                            </span>
                          </div>
                        </div>
                      </div>
                      <div className="flex items-center gap-2">
                        <button
                          onClick={e => { e.stopPropagation(); setRunCrewDialog(crew.id || null); setCrewTaskInput(''); setCrewResult(null); }}
                          className="flex items-center gap-1 px-2.5 py-1 rounded-lg text-[10px] font-medium"
                          style={{ backgroundColor: `${colors.success}15`, color: colors.success }}
                        >
                          <Play className="w-3 h-3" /> Run
                        </button>
                        <button
                          onClick={e => { e.stopPropagation(); if (crew.id) handleDeleteCrew(crew.id); }}
                          className="p-1 rounded hover:opacity-80"
                          style={{ color: colors.error }}
                        >
                          <Trash2 className="w-3.5 h-3.5" />
                        </button>
                        {isExpanded ? <ChevronDown className="w-4 h-4" style={{ color: colors.textMuted }} /> : <ChevronRight className="w-4 h-4" style={{ color: colors.textMuted }} />}
                      </div>
                    </div>

                    {isExpanded && (
                      <div className="px-4 pb-3 space-y-2" style={{ borderTop: `1px solid ${colors.border}` }}>
                        {crew.description && (
                          <p className="text-xs pt-2" style={{ color: colors.textMuted }}>{crew.description}</p>
                        )}
                        <div className="space-y-1.5 pt-1">
                          {crew.agents.sort((a, b) => a.order - b.order).map((member, idx) => {
                            const agentInfo = agents.find(a => a.id === member.agent_id);
                            return (
                              <div key={member.agent_id} className="flex items-center gap-2 px-2 py-1.5 rounded-lg" style={{ backgroundColor: colors.bgTertiary }}>
                                <span className="text-[10px] font-medium px-1.5 py-0.5 rounded" style={{ backgroundColor: `${colors.primary}15`, color: colors.primary }}>
                                  #{idx + 1}
                                </span>
                                <span className="text-xs font-medium" style={{ color: colors.text }}>
                                  {agentInfo?.name || member.agent_id}
                                </span>
                                {crew.process === 'hierarchical' && crew.coordinator_id === member.agent_id && (
                                  <Crown className="w-3 h-3" style={{ color: colors.warning }} />
                                )}
                                <span className="text-[10px] px-1.5 py-0.5 rounded-full" style={{ backgroundColor: `${colors.secondary || '#6366f1'}15`, color: colors.secondary || '#6366f1' }}>
                                  {member.role}
                                </span>
                                {member.goal && (
                                  <span className="text-[10px] truncate flex-1" style={{ color: colors.textMuted }}>
                                    {member.goal}
                                  </span>
                                )}
                                {idx < crew.agents.length - 1 && crew.process === 'sequential' && (
                                  <ArrowRight className="w-3 h-3 flex-shrink-0" style={{ color: colors.textMuted }} />
                                )}
                              </div>
                            );
                          })}
                        </div>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {/* Execution Logs */}
        <div>
          <h3 className="text-sm font-semibold mb-3 flex items-center gap-2" style={{ color: colors.text }}>
            <Activity className="w-4 h-4" style={{ color: colors.secondary || '#6366f1' }} />
            Execution Logs
            <span className="text-[10px] font-normal" style={{ color: colors.textMuted }}>
              (last 50)
            </span>
          </h3>
          {recentExecs.length === 0 ? (
            <div
              className="rounded-xl border p-6 text-center"
              style={{ backgroundColor: colors.bgSecondary, borderColor: colors.border }}
            >
              <Clock className="w-6 h-6 mx-auto mb-2" style={{ color: colors.textMuted }} />
              <p className="text-xs" style={{ color: colors.textMuted }}>
                No executions recorded yet. Agent logs will appear here as agents process queries.
              </p>
            </div>
          ) : (
            <div
              className="rounded-xl border overflow-hidden"
              style={{ backgroundColor: colors.bgSecondary, borderColor: colors.border }}
            >
              {/* Table header */}
              <div
                className="grid grid-cols-[1fr_2fr_80px_60px_80px] gap-2 px-4 py-2 text-[10px] font-semibold border-b"
                style={{ color: colors.textMuted, borderColor: colors.border }}
              >
                <span>AGENT</span>
                <span>QUERY</span>
                <span>STATUS</span>
                <span>TIME</span>
                <span>WHEN</span>
              </div>
              {/* Rows */}
              <div className="divide-y" style={{ borderColor: colors.border }}>
                {recentExecs.map(exec => {
                  const isLogExpanded = expandedLog === exec.id;
                  return (
                    <div key={exec.id}>
                      <button
                        onClick={() => setExpandedLog(isLogExpanded ? null : exec.id)}
                        className="w-full grid grid-cols-[1fr_2fr_80px_60px_80px] gap-2 px-4 py-2.5 text-xs items-center transition-colors"
                        style={{ color: colors.text }}
                        onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                        onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                      >
                        <span className="truncate font-medium">{exec.agent_name}</span>
                        <span className="truncate" style={{ color: colors.textSecondary }}>
                          {exec.query}
                        </span>
                        <span className="flex items-center gap-1">
                          {exec.status === 'completed' && exec.success ? (
                            <CheckCircle2 className="w-3 h-3" style={{ color: '#10b981' }} />
                          ) : exec.status === 'failed' || !exec.success ? (
                            <XCircle className="w-3 h-3" style={{ color: colors.error }} />
                          ) : exec.status === 'running' ? (
                            <Loader2 className="w-3 h-3 animate-spin" style={{ color: colors.warning }} />
                          ) : (
                            <Circle className="w-3 h-3" style={{ color: colors.textMuted }} />
                          )}
                          <span
                            className="text-[10px]"
                            style={{
                              color: exec.success ? '#10b981' : colors.error,
                            }}
                          >
                            {exec.status}
                          </span>
                        </span>
                        <span className="font-mono text-[10px]" style={{ color: colors.textMuted }}>
                          {exec.execution_time_ms
                            ? exec.execution_time_ms > 1000
                              ? `${(exec.execution_time_ms / 1000).toFixed(1)}s`
                              : `${exec.execution_time_ms}ms`
                            : '—'}
                        </span>
                        <span className="text-[10px]" style={{ color: colors.textMuted }}>
                          {relativeTimeShort(exec.started_at)}
                        </span>
                      </button>

                      <AnimatePresence>
                        {isLogExpanded && (
                          <motion.div
                            initial={{ height: 0, opacity: 0 }}
                            animate={{ height: 'auto', opacity: 1 }}
                            exit={{ height: 0, opacity: 0 }}
                            className="overflow-hidden"
                          >
                            <div
                              className="px-4 pb-3 space-y-3"
                              style={{ backgroundColor: `${colors.bgTertiary}50` }}
                            >
                              {/* Agent Response */}
                              {exec.response && (
                                <div>
                                  <span className="text-[10px] font-semibold block mb-1.5" style={{ color: colors.textMuted }}>
                                    RESPONSE
                                  </span>
                                  <div
                                    className="text-xs leading-relaxed p-3 rounded-lg whitespace-pre-wrap max-h-64 overflow-y-auto"
                                    style={{
                                      backgroundColor: colors.bgSecondary,
                                      color: colors.text,
                                      border: `1px solid ${colors.border}`,
                                    }}
                                  >
                                    {exec.response}
                                  </div>
                                </div>
                              )}

                              {/* Query */}
                              <div>
                                <span className="text-[10px] font-semibold block mb-1" style={{ color: colors.textMuted }}>
                                  QUERY
                                </span>
                                <div className="text-xs" style={{ color: colors.textSecondary }}>
                                  {exec.query}
                                </div>
                              </div>

                              {/* Execution Stats */}
                              <div className="flex items-center gap-4 flex-wrap">
                                {exec.steps_count > 0 && (
                                  <span className="text-[10px] flex items-center gap-1" style={{ color: colors.textMuted }}>
                                    <Activity className="w-2.5 h-2.5" />
                                    {exec.steps_count} steps
                                  </span>
                                )}
                                {exec.execution_time_ms && (
                                  <span className="text-[10px] flex items-center gap-1" style={{ color: colors.textMuted }}>
                                    <Clock className="w-2.5 h-2.5" />
                                    {exec.execution_time_ms > 1000
                                      ? `${(exec.execution_time_ms / 1000).toFixed(1)}s`
                                      : `${exec.execution_time_ms}ms`}
                                  </span>
                                )}
                              </div>

                              {/* Tools Used */}
                              {exec.tools_used.length > 0 && (
                                <div>
                                  <span className="text-[10px] font-semibold block mb-1" style={{ color: colors.textMuted }}>
                                    TOOLS USED
                                  </span>
                                  <div className="flex flex-wrap gap-1">
                                    {exec.tools_used.map((tool, i) => (
                                      <span
                                        key={i}
                                        className="text-[10px] px-1.5 py-px rounded-full flex items-center gap-0.5"
                                        style={{
                                          backgroundColor: `${colors.secondary || '#6366f1'}12`,
                                          color: colors.secondary || '#6366f1',
                                        }}
                                      >
                                        <Wrench className="w-2.5 h-2.5" />
                                        {tool}
                                      </span>
                                    ))}
                                  </div>
                                </div>
                              )}

                              {/* Error */}
                              {exec.error_message && (
                                <div>
                                  <span className="text-[10px] font-semibold" style={{ color: colors.error }}>
                                    ERROR
                                  </span>
                                  <pre
                                    className="text-[10px] mt-1 p-2 rounded-lg overflow-x-auto"
                                    style={{
                                      backgroundColor: `${colors.error}08`,
                                      color: colors.error,
                                      border: `1px solid ${colors.error}20`,
                                    }}
                                  >
                                    {exec.error_message}
                                  </pre>
                                </div>
                              )}

                              <div className="text-[10px] font-mono pt-1" style={{ color: colors.textMuted }}>
                                ID: {exec.id}
                              </div>
                            </div>
                          </motion.div>
                        )}
                      </AnimatePresence>
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Agent Builder Modal */}
      <AgentBuilder
        isOpen={showBuilder}
        onClose={() => { setShowBuilder(false); setEditingAgent(null); }}
        colors={colors}
        theme={theme}
        onAgentCreated={handleAgentCreated}
        editAgent={editingAgent}
      />

      {/* Run Agent Dialog */}
      <AnimatePresence>
        {runDialogAgent && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-50 flex items-center justify-center"
            style={{ background: 'rgba(0,0,0,0.5)', backdropFilter: 'blur(4px)' }}
            onClick={(e) => { if (e.target === e.currentTarget) { setRunDialogAgent(null); setExecutionResult(null); } }}
          >
            <motion.div
              initial={{ opacity: 0, scale: 0.95, y: 20 }}
              animate={{ opacity: 1, scale: 1, y: 0 }}
              exit={{ opacity: 0, scale: 0.95, y: 20 }}
              className="w-full max-w-xl max-h-[80vh] rounded-2xl overflow-hidden flex flex-col"
              style={{ background: colors.bgPrimary, border: `1px solid ${colors.border}` }}
            >
              {/* Header */}
              <div className="flex items-center justify-between px-5 py-3.5" style={{ borderBottom: `1px solid ${colors.border}` }}>
                <div className="flex items-center gap-2.5">
                  <div className="w-7 h-7 rounded-lg flex items-center justify-center" style={{ background: '#22c55e15' }}>
                    <Play className="w-3.5 h-3.5" style={{ color: '#22c55e' }} />
                  </div>
                  <div>
                    <h3 className="text-sm font-semibold" style={{ color: colors.text }}>
                      Run Agent: {agents.find(a => a.id === runDialogAgent)?.name}
                    </h3>
                    <p className="text-[10px]" style={{ color: colors.textMuted }}>
                      Enter a query to execute the agent
                    </p>
                  </div>
                </div>
              </div>

              {/* Query Input */}
              <div className="px-5 py-4">
                <div className="flex gap-2">
                  <input
                    value={runQuery}
                    onChange={(e) => setRunQuery(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && !e.shiftKey && runDialogAgent) {
                        handleRunAgent(runDialogAgent);
                      }
                    }}
                    placeholder="Ask the agent something..."
                    className="flex-1 px-3.5 py-2.5 rounded-lg text-sm outline-none focus:ring-2"
                    style={{
                      background: colors.bgTertiary,
                      color: colors.text,
                      border: `1px solid ${colors.border}`,
                    }}
                    autoFocus
                    disabled={runningExecution}
                  />
                  <button
                    onClick={() => runDialogAgent && handleRunAgent(runDialogAgent)}
                    disabled={runningExecution || !runQuery.trim()}
                    className="px-4 py-2.5 rounded-lg text-xs font-medium text-white flex items-center gap-1.5 disabled:opacity-50 transition-opacity"
                    style={{ backgroundColor: colors.primary }}
                  >
                    {runningExecution ? (
                      <Loader2 className="w-3.5 h-3.5 animate-spin" />
                    ) : (
                      <Send className="w-3.5 h-3.5" />
                    )}
                    {runningExecution ? 'Running...' : 'Run'}
                  </button>
                </div>
              </div>

              {/* Execution Result */}
              {executionResult && (
                <div className="px-5 pb-4 overflow-y-auto flex-1">
                  <div
                    className="rounded-xl border p-4"
                    style={{
                      backgroundColor: executionResult.success ? `${colors.bgSecondary}` : `${colors.error}08`,
                      borderColor: executionResult.success ? colors.border : `${colors.error}30`,
                    }}
                  >
                    {/* Status header */}
                    <div className="flex items-center justify-between mb-3">
                      <div className="flex items-center gap-2">
                        {executionResult.success ? (
                          <CheckCircle2 className="w-4 h-4" style={{ color: '#10b981' }} />
                        ) : (
                          <XCircle className="w-4 h-4" style={{ color: colors.error }} />
                        )}
                        <span className="text-xs font-semibold" style={{ color: executionResult.success ? '#10b981' : colors.error }}>
                          {executionResult.success ? 'Execution Successful' : 'Execution Failed'}
                        </span>
                      </div>
                      <div className="flex items-center gap-3 text-[10px]" style={{ color: colors.textMuted }}>
                        <span>{executionResult.execution_time_ms}ms</span>
                        {executionResult.steps.length > 0 && (
                          <span>{executionResult.steps.length} steps</span>
                        )}
                        {executionResult.tools_used.length > 0 && (
                          <span>{executionResult.tools_used.length} tools</span>
                        )}
                      </div>
                    </div>

                    {/* Response */}
                    {executionResult.response && (
                      <div
                        className="text-sm leading-relaxed whitespace-pre-wrap"
                        style={{ color: colors.text }}
                      >
                        {executionResult.response}
                      </div>
                    )}

                    {/* Error */}
                    {executionResult.error && (
                      <pre
                        className="text-xs mt-2 p-2.5 rounded-lg overflow-x-auto"
                        style={{
                          backgroundColor: `${colors.error}08`,
                          color: colors.error,
                          border: `1px solid ${colors.error}20`,
                        }}
                      >
                        {executionResult.error}
                      </pre>
                    )}

                    {/* Tools used */}
                    {executionResult.tools_used.length > 0 && (
                      <div className="mt-3 flex flex-wrap gap-1">
                        {executionResult.tools_used.map((tool, i) => (
                          <span
                            key={i}
                            className="text-[10px] px-1.5 py-px rounded-full flex items-center gap-0.5"
                            style={{
                              backgroundColor: `${colors.primary}12`,
                              color: colors.primary,
                            }}
                          >
                            <Wrench className="w-2.5 h-2.5" />
                            {tool}
                          </span>
                        ))}
                      </div>
                    )}
                  </div>
                </div>
              )}

              {/* Footer */}
              <div className="flex items-center justify-end px-5 py-3" style={{ borderTop: `1px solid ${colors.border}` }}>
                <button
                  onClick={() => { setRunDialogAgent(null); setExecutionResult(null); }}
                  className="px-4 py-2 rounded-lg text-xs font-medium transition-opacity hover:opacity-70"
                  style={{ color: colors.textMuted }}
                >
                  Close
                </button>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>

      {/* Crew Builder Modal */}
      <CrewBuilder
        isOpen={showCrewBuilder}
        onClose={() => setShowCrewBuilder(false)}
        onCrewCreated={() => { fetchData(); setShowCrewBuilder(false); }}
      />

      {/* Run Crew Dialog */}
      <AnimatePresence>
        {runCrewDialog && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-50 flex items-center justify-center"
            style={{ background: 'rgba(0,0,0,0.5)', backdropFilter: 'blur(4px)' }}
            onClick={(e) => { if (e.target === e.currentTarget) { setRunCrewDialog(null); setCrewResult(null); } }}
          >
            <motion.div
              initial={{ opacity: 0, scale: 0.95, y: 20 }}
              animate={{ opacity: 1, scale: 1, y: 0 }}
              exit={{ opacity: 0, scale: 0.95, y: 20 }}
              className="w-full max-w-2xl max-h-[85vh] rounded-2xl overflow-hidden flex flex-col"
              style={{ background: colors.bgPrimary, border: `1px solid ${colors.border}` }}
            >
              {/* Header */}
              <div className="flex items-center justify-between px-5 py-3.5" style={{ borderBottom: `1px solid ${colors.border}` }}>
                <div className="flex items-center gap-2.5">
                  <div className="w-7 h-7 rounded-lg flex items-center justify-center" style={{ background: `${colors.secondary || '#6366f1'}15` }}>
                    <Users className="w-3.5 h-3.5" style={{ color: colors.secondary || '#6366f1' }} />
                  </div>
                  <div>
                    <h3 className="text-sm font-semibold" style={{ color: colors.text }}>
                      Run Crew: {crews.find(c => c.id === runCrewDialog)?.name}
                    </h3>
                    <p className="text-[10px]" style={{ color: colors.textMuted }}>
                      Enter a task for the crew to work on collaboratively
                    </p>
                  </div>
                </div>
              </div>

              {/* Task Input */}
              <div className="px-5 py-4">
                <div className="flex gap-2">
                  <input
                    value={crewTaskInput}
                    onChange={(e) => setCrewTaskInput(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && !e.shiftKey && runCrewDialog) {
                        handleRunCrew(runCrewDialog);
                      }
                    }}
                    placeholder="Describe the task for the crew..."
                    className="flex-1 px-3.5 py-2.5 rounded-lg text-sm outline-none focus:ring-2"
                    style={{ background: colors.bgTertiary, color: colors.text, border: `1px solid ${colors.border}` }}
                    autoFocus
                    disabled={runningCrew}
                  />
                  <button
                    onClick={() => runCrewDialog && handleRunCrew(runCrewDialog)}
                    disabled={runningCrew || !crewTaskInput.trim()}
                    className="px-4 py-2.5 rounded-lg text-xs font-medium text-white flex items-center gap-1.5 disabled:opacity-50 transition-opacity"
                    style={{ backgroundColor: colors.primary }}
                  >
                    {runningCrew ? (
                      <Loader2 className="w-3.5 h-3.5 animate-spin" />
                    ) : (
                      <Send className="w-3.5 h-3.5" />
                    )}
                    {runningCrew ? 'Running...' : 'Run Crew'}
                  </button>
                </div>
              </div>

              {/* Crew Execution Result */}
              {crewResult && (
                <div className="px-5 pb-4 overflow-y-auto flex-1">
                  <div
                    className="rounded-xl border p-4 space-y-3"
                    style={{
                      backgroundColor: crewResult.success ? colors.bgSecondary : `${colors.error}08`,
                      borderColor: crewResult.success ? colors.border : `${colors.error}30`,
                    }}
                  >
                    {/* Status */}
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-2">
                        {crewResult.success ? (
                          <CheckCircle2 className="w-4 h-4" style={{ color: '#10b981' }} />
                        ) : (
                          <XCircle className="w-4 h-4" style={{ color: colors.error }} />
                        )}
                        <span className="text-xs font-semibold" style={{ color: crewResult.success ? '#10b981' : colors.error }}>
                          {crewResult.success ? 'Crew Execution Complete' : 'Crew Execution Failed'}
                        </span>
                      </div>
                      <span className="text-[10px]" style={{ color: colors.textMuted }}>
                        {crewResult.execution_time_ms}ms | {crewResult.agent_outputs.length} agents
                      </span>
                    </div>

                    {/* Error */}
                    {crewResult.error && (
                      <pre className="text-xs p-2.5 rounded-lg overflow-x-auto" style={{ backgroundColor: `${colors.error}08`, color: colors.error, border: `1px solid ${colors.error}20` }}>
                        {crewResult.error}
                      </pre>
                    )}

                    {/* Per-agent outputs */}
                    {crewResult.agent_outputs.length > 0 && (
                      <div className="space-y-2">
                        <span className="text-[10px] font-semibold uppercase tracking-wider" style={{ color: colors.textMuted }}>Agent Outputs</span>
                        {crewResult.agent_outputs.map((ao, idx) => (
                          <div key={idx} className="rounded-lg p-3 space-y-1.5" style={{ backgroundColor: colors.bgTertiary, border: `1px solid ${colors.border}` }}>
                            <div className="flex items-center justify-between">
                              <div className="flex items-center gap-2">
                                <span className="text-[10px] font-medium px-1.5 py-0.5 rounded" style={{ backgroundColor: `${colors.primary}15`, color: colors.primary }}>
                                  #{idx + 1}
                                </span>
                                <span className="text-xs font-medium" style={{ color: colors.text }}>{ao.agent_name}</span>
                                <span className="text-[10px] px-1.5 py-0.5 rounded-full" style={{ backgroundColor: `${colors.secondary || '#6366f1'}15`, color: colors.secondary || '#6366f1' }}>
                                  {ao.role}
                                </span>
                              </div>
                              <span className="text-[10px]" style={{ color: colors.textMuted }}>{ao.execution_time_ms}ms</span>
                            </div>
                            <div className="text-xs leading-relaxed whitespace-pre-wrap max-h-40 overflow-y-auto" style={{ color: colors.text }}>
                              {ao.output}
                            </div>
                            {ao.tools_used.length > 0 && (
                              <div className="flex flex-wrap gap-1">
                                {ao.tools_used.map((tool, i) => (
                                  <span key={i} className="text-[10px] px-1.5 py-px rounded-full" style={{ backgroundColor: `${colors.primary}12`, color: colors.primary }}>
                                    {tool}
                                  </span>
                                ))}
                              </div>
                            )}
                          </div>
                        ))}
                      </div>
                    )}

                    {/* Final output */}
                    {crewResult.final_output && (
                      <div className="space-y-1">
                        <span className="text-[10px] font-semibold uppercase tracking-wider" style={{ color: colors.textMuted }}>Final Output</span>
                        <div className="text-sm leading-relaxed whitespace-pre-wrap max-h-60 overflow-y-auto p-3 rounded-lg" style={{ backgroundColor: colors.bgPrimary, color: colors.text, border: `1px solid ${colors.border}` }}>
                          {crewResult.final_output}
                        </div>
                      </div>
                    )}
                  </div>
                </div>
              )}

              {/* Footer */}
              <div className="flex items-center justify-end px-5 py-3" style={{ borderTop: `1px solid ${colors.border}` }}>
                <button
                  onClick={() => { setRunCrewDialog(null); setCrewResult(null); }}
                  className="px-4 py-2 rounded-lg text-xs font-medium transition-opacity hover:opacity-70"
                  style={{ color: colors.textMuted }}
                >
                  Close
                </button>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
