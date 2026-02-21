import React, { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  X, Users, ArrowRight, GripVertical, Plus, Trash2,
  GitBranch, ListOrdered, Crown, Save, AlertCircle,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useTheme } from '../contexts/ThemeContext';

interface AgentInfo {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
}

interface CrewMember {
  agent_id: string;
  role: string;
  goal: string;
  order: number;
}

interface CrewDefinition {
  id?: string;
  name: string;
  description: string;
  agents: CrewMember[];
  process: string;
  coordinator_id?: string;
  config: {
    timeout_seconds: number;
    verbose: boolean;
  };
}

interface CrewBuilderProps {
  isOpen: boolean;
  onClose: () => void;
  onCrewCreated: (crewId: string) => void;
  editingCrew?: CrewDefinition | null;
}

export function CrewBuilder({ isOpen, onClose, onCrewCreated, editingCrew }: CrewBuilderProps) {
  const { colors } = useTheme();
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [process, setProcess] = useState<'sequential' | 'hierarchical'>('sequential');
  const [members, setMembers] = useState<CrewMember[]>([]);
  const [coordinatorId, setCoordinatorId] = useState<string>('');
  const [timeout, setTimeout] = useState(300);
  const [availableAgents, setAvailableAgents] = useState<AgentInfo[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Load available agents
  useEffect(() => {
    if (isOpen) {
      invoke<AgentInfo[]>('list_agents')
        .then(agents => setAvailableAgents(agents.filter(a => a.enabled)))
        .catch(err => console.error('Failed to load agents:', err));
    }
  }, [isOpen]);

  // Pre-fill if editing
  useEffect(() => {
    if (editingCrew) {
      setName(editingCrew.name);
      setDescription(editingCrew.description);
      setProcess(editingCrew.process as 'sequential' | 'hierarchical');
      setMembers(editingCrew.agents);
      setCoordinatorId(editingCrew.coordinator_id || '');
      setTimeout(editingCrew.config.timeout_seconds);
    } else {
      setName('');
      setDescription('');
      setProcess('sequential');
      setMembers([]);
      setCoordinatorId('');
      setTimeout(300);
    }
    setError(null);
  }, [editingCrew, isOpen]);

  const addMember = (agentId: string) => {
    if (members.some(m => m.agent_id === agentId)) return;
    const agent = availableAgents.find(a => a.id === agentId);
    if (!agent) return;
    setMembers(prev => [...prev, {
      agent_id: agentId,
      role: '',
      goal: '',
      order: prev.length,
    }]);
  };

  const removeMember = (agentId: string) => {
    setMembers(prev => prev.filter(m => m.agent_id !== agentId).map((m, i) => ({ ...m, order: i })));
    if (coordinatorId === agentId) setCoordinatorId('');
  };

  const updateMember = (agentId: string, field: 'role' | 'goal', value: string) => {
    setMembers(prev => prev.map(m => m.agent_id === agentId ? { ...m, [field]: value } : m));
  };

  const moveMember = (fromIdx: number, toIdx: number) => {
    if (toIdx < 0 || toIdx >= members.length) return;
    const updated = [...members];
    const [moved] = updated.splice(fromIdx, 1);
    updated.splice(toIdx, 0, moved);
    setMembers(updated.map((m, i) => ({ ...m, order: i })));
  };

  const handleSave = async () => {
    setError(null);

    if (!name.trim()) { setError('Crew name is required'); return; }
    if (members.length < 2) { setError('A crew needs at least 2 agents'); return; }
    if (members.some(m => !m.role.trim())) { setError('Every member needs a role'); return; }
    if (process === 'hierarchical' && !coordinatorId) { setError('Select a coordinator for hierarchical mode'); return; }

    setSaving(true);
    try {
      const crew: CrewDefinition = {
        id: editingCrew?.id,
        name: name.trim(),
        description: description.trim(),
        agents: members,
        process,
        coordinator_id: process === 'hierarchical' ? coordinatorId : undefined,
        config: { timeout_seconds: timeout, verbose: false },
      };

      const crewId = await invoke<string>('create_crew', { crew });
      onCrewCreated(crewId);
      onClose();
    } catch (err: any) {
      setError(err?.toString() || 'Failed to save crew');
    } finally {
      setSaving(false);
    }
  };

  const unselectedAgents = availableAgents.filter(a => !members.some(m => m.agent_id === a.id));

  if (!isOpen) return null;

  return (
    <AnimatePresence>
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        className="fixed inset-0 z-50 flex items-center justify-center"
        style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
        onClick={onClose}
      >
        <motion.div
          initial={{ scale: 0.95, opacity: 0 }}
          animate={{ scale: 1, opacity: 1 }}
          exit={{ scale: 0.95, opacity: 0 }}
          className="w-full max-w-2xl max-h-[85vh] rounded-xl overflow-hidden flex flex-col"
          style={{ backgroundColor: colors.bgPrimary, border: `1px solid ${colors.border}` }}
          onClick={e => e.stopPropagation()}
        >
          {/* Header */}
          <div className="flex items-center justify-between px-6 py-4" style={{ borderBottom: `1px solid ${colors.border}` }}>
            <div className="flex items-center gap-3">
              <div className="w-8 h-8 rounded-lg flex items-center justify-center" style={{ backgroundColor: `${colors.primary}15` }}>
                <Users className="w-4 h-4" style={{ color: colors.primary }} />
              </div>
              <h2 className="text-base font-semibold" style={{ color: colors.text }}>
                {editingCrew ? 'Edit Crew' : 'Create Crew'}
              </h2>
            </div>
            <button onClick={onClose} className="p-1 rounded-md hover:opacity-80" style={{ color: colors.textMuted }}>
              <X className="w-5 h-5" />
            </button>
          </div>

          {/* Content */}
          <div className="flex-1 overflow-y-auto px-6 py-4 space-y-5">
            {/* Name & Description */}
            <div className="space-y-3">
              <div>
                <label className="text-xs font-medium mb-1 block" style={{ color: colors.textMuted }}>Crew Name</label>
                <input
                  value={name}
                  onChange={e => setName(e.target.value)}
                  placeholder="e.g. Research & Writing Team"
                  className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                  style={{ backgroundColor: colors.bgSecondary, color: colors.text, border: `1px solid ${colors.border}` }}
                />
              </div>
              <div>
                <label className="text-xs font-medium mb-1 block" style={{ color: colors.textMuted }}>Description</label>
                <input
                  value={description}
                  onChange={e => setDescription(e.target.value)}
                  placeholder="What does this crew do?"
                  className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                  style={{ backgroundColor: colors.bgSecondary, color: colors.text, border: `1px solid ${colors.border}` }}
                />
              </div>
            </div>

            {/* Process Type */}
            <div>
              <label className="text-xs font-medium mb-2 block" style={{ color: colors.textMuted }}>Execution Process</label>
              <div className="flex gap-2">
                <button
                  onClick={() => setProcess('sequential')}
                  className="flex-1 flex items-center gap-2 px-3 py-2.5 rounded-lg text-xs font-medium transition-all"
                  style={{
                    backgroundColor: process === 'sequential' ? `${colors.primary}15` : colors.bgSecondary,
                    color: process === 'sequential' ? colors.primary : colors.textMuted,
                    border: `1px solid ${process === 'sequential' ? colors.primary : colors.border}`,
                  }}
                >
                  <ListOrdered className="w-4 h-4" />
                  Sequential
                  <span className="text-[10px] opacity-70">A → B → C</span>
                </button>
                <button
                  onClick={() => setProcess('hierarchical')}
                  className="flex-1 flex items-center gap-2 px-3 py-2.5 rounded-lg text-xs font-medium transition-all"
                  style={{
                    backgroundColor: process === 'hierarchical' ? `${colors.primary}15` : colors.bgSecondary,
                    color: process === 'hierarchical' ? colors.primary : colors.textMuted,
                    border: `1px solid ${process === 'hierarchical' ? colors.primary : colors.border}`,
                  }}
                >
                  <GitBranch className="w-4 h-4" />
                  Hierarchical
                  <span className="text-[10px] opacity-70">Coordinator delegates</span>
                </button>
              </div>
            </div>

            {/* Members */}
            <div>
              <div className="flex items-center justify-between mb-2">
                <label className="text-xs font-medium" style={{ color: colors.textMuted }}>
                  Team Members ({members.length})
                </label>
              </div>

              {/* Member list */}
              <div className="space-y-2 mb-3">
                {members.map((member, idx) => {
                  const agent = availableAgents.find(a => a.id === member.agent_id);
                  return (
                    <div
                      key={member.agent_id}
                      className="rounded-lg p-3 space-y-2"
                      style={{ backgroundColor: colors.bgSecondary, border: `1px solid ${colors.border}` }}
                    >
                      <div className="flex items-center justify-between">
                        <div className="flex items-center gap-2">
                          <div className="flex flex-col gap-0.5">
                            <button
                              onClick={() => moveMember(idx, idx - 1)}
                              disabled={idx === 0}
                              className="text-[10px] opacity-50 hover:opacity-100 disabled:opacity-20"
                              style={{ color: colors.textMuted }}
                            >
                              ▲
                            </button>
                            <button
                              onClick={() => moveMember(idx, idx + 1)}
                              disabled={idx === members.length - 1}
                              className="text-[10px] opacity-50 hover:opacity-100 disabled:opacity-20"
                              style={{ color: colors.textMuted }}
                            >
                              ▼
                            </button>
                          </div>
                          <span className="text-xs font-medium px-1.5 py-0.5 rounded" style={{ backgroundColor: `${colors.primary}15`, color: colors.primary }}>
                            #{idx + 1}
                          </span>
                          <span className="text-sm font-medium" style={{ color: colors.text }}>
                            {agent?.name || member.agent_id}
                          </span>
                          {process === 'hierarchical' && coordinatorId === member.agent_id && (
                            <span className="flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded-full" style={{ backgroundColor: `${colors.warning}20`, color: colors.warning }}>
                              <Crown className="w-3 h-3" /> Coordinator
                            </span>
                          )}
                        </div>
                        <div className="flex items-center gap-1">
                          {process === 'hierarchical' && coordinatorId !== member.agent_id && (
                            <button
                              onClick={() => setCoordinatorId(member.agent_id)}
                              className="text-[10px] px-2 py-1 rounded hover:opacity-80"
                              style={{ color: colors.textMuted, backgroundColor: colors.bgTertiary }}
                              title="Set as coordinator"
                            >
                              <Crown className="w-3 h-3" />
                            </button>
                          )}
                          <button
                            onClick={() => removeMember(member.agent_id)}
                            className="p-1 rounded hover:opacity-80"
                            style={{ color: colors.error }}
                          >
                            <Trash2 className="w-3.5 h-3.5" />
                          </button>
                        </div>
                      </div>
                      <div className="flex gap-2">
                        <input
                          value={member.role}
                          onChange={e => updateMember(member.agent_id, 'role', e.target.value)}
                          placeholder="Role (e.g. researcher)"
                          className="flex-1 px-2 py-1.5 rounded text-xs outline-none"
                          style={{ backgroundColor: colors.bgTertiary, color: colors.text, border: `1px solid ${colors.border}` }}
                        />
                        <input
                          value={member.goal}
                          onChange={e => updateMember(member.agent_id, 'goal', e.target.value)}
                          placeholder="Goal for this agent"
                          className="flex-[2] px-2 py-1.5 rounded text-xs outline-none"
                          style={{ backgroundColor: colors.bgTertiary, color: colors.text, border: `1px solid ${colors.border}` }}
                        />
                      </div>
                    </div>
                  );
                })}
              </div>

              {/* Add agent */}
              {unselectedAgents.length > 0 && (
                <div className="flex flex-wrap gap-1.5">
                  {unselectedAgents.map(agent => (
                    <button
                      key={agent.id}
                      onClick={() => addMember(agent.id)}
                      className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs hover:opacity-80 transition-opacity"
                      style={{ backgroundColor: colors.bgTertiary, color: colors.textMuted, border: `1px solid ${colors.border}` }}
                    >
                      <Plus className="w-3 h-3" />
                      {agent.name}
                    </button>
                  ))}
                </div>
              )}

              {availableAgents.length === 0 && (
                <p className="text-xs text-center py-4" style={{ color: colors.textMuted }}>
                  No agents available. Create agents first.
                </p>
              )}
            </div>

            {/* Timeout */}
            <div>
              <label className="text-xs font-medium mb-1 block" style={{ color: colors.textMuted }}>
                Timeout (seconds): {timeout}
              </label>
              <input
                type="range"
                min={60}
                max={600}
                step={30}
                value={timeout}
                onChange={e => setTimeout(Number(e.target.value))}
                className="w-full"
              />
            </div>

            {/* Error */}
            {error && (
              <div className="flex items-center gap-2 px-3 py-2 rounded-lg text-xs" style={{ backgroundColor: `${colors.error}15`, color: colors.error }}>
                <AlertCircle className="w-4 h-4 flex-shrink-0" />
                {error}
              </div>
            )}
          </div>

          {/* Footer */}
          <div className="flex items-center justify-end gap-3 px-6 py-4" style={{ borderTop: `1px solid ${colors.border}` }}>
            <button
              onClick={onClose}
              className="px-4 py-2 rounded-lg text-xs font-medium"
              style={{ color: colors.textMuted }}
            >
              Cancel
            </button>
            <button
              onClick={handleSave}
              disabled={saving}
              className="flex items-center gap-2 px-4 py-2 rounded-lg text-xs font-medium text-white transition-opacity disabled:opacity-50"
              style={{ backgroundColor: colors.primary }}
            >
              {saving ? <span className="animate-spin">...</span> : <Save className="w-3.5 h-3.5" />}
              {editingCrew ? 'Update Crew' : 'Create Crew'}
            </button>
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  );
}
