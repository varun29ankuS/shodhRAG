/**
 * Tauri commands of Settings → Tools & connections and the composer's tool
 * chip (`mcp_commands.rs`). Every call is the user's own action: the agent
 * has no tool for any of them.
 */
import { invoke } from '@tauri-apps/api/core';

export type ToolMode = 'research' | 'code';
export type Approval = 'ask' | 'auto';
export type ConfigScope = 'global' | 'workspace';

export interface ToolView {
  name: string;
  title: string | null;
  description: string | null;
  readOnly: boolean;
  enabled: boolean;
  approval: Approval;
}

export interface ServerView {
  name: string;
  kind: 'stdio' | 'http';
  /** The command line or URL (never environment values or headers). */
  target: string;
  enabled: boolean;
  /** Empty: every mode. */
  modes: ToolMode[];
  status: 'connected' | 'error' | 'unknown';
  error: string | null;
  tools: ToolView[];
  /** Runs in the workspace's code folder. */
  needsFolder: boolean;
}

export interface ServerProblem {
  name: string;
  error: string;
}

export interface BuiltinTool {
  name: string;
  label: string;
  description: string;
  tier: 'read' | 'write' | 'destructive';
  lastUsed: string | null;
}

export interface BuiltinGroup {
  group: string;
  tools: BuiltinTool[];
}

export interface SkillView {
  name: string;
  description: string;
  enabled: boolean;
  files: number;
  /** Empty: every mode. */
  modes: ToolMode[];
  license: string | null;
  repo: string | null;
}

export interface RecommendedView {
  id: string;
  title: string;
  description: string;
  repo: string;
  license: string;
  skills: string[];
  modes: ToolMode[];
  installed: boolean;
}

export interface SkillProblem {
  folder: string;
  error: string;
}

export interface ToolsOverview {
  scope: ConfigScope;
  configPath: string;
  servers: ServerView[];
  problems: ServerProblem[];
  configError: string | null;
  builtin: BuiltinGroup[];
  skills: SkillView[];
  skillProblems: SkillProblem[];
  recommended: RecommendedView[];
  enola: { version: string; supported: boolean; installed: boolean; registered: boolean };
}

export interface StagedSkill {
  name: string;
  description: string;
  files: number;
  replaces: boolean;
}

export interface StagedInstall {
  token: string;
  source: string;
  skills: StagedSkill[];
  problems: SkillProblem[];
}

export interface ChatServer {
  name: string;
  scope: ConfigScope;
  error: string | null;
  tools: string[];
}

export interface ChatToolsView {
  servers: ChatServer[];
  skills: { name: string; description: string }[];
  builtin: number;
  total: number;
  many: boolean;
}

export const toolsApi = {
  overview: (workspaceId: string | null) => invoke<ToolsOverview>('tools_overview', { workspaceId }),
  testServer: (workspaceId: string | null, name: string) =>
    invoke<ServerView>('mcp_test_server', { workspaceId, name }),
  addServers: (workspaceId: string | null, configText: string) =>
    invoke<string[]>('mcp_add_servers', { workspaceId, configText }),
  removeServer: (workspaceId: string | null, name: string) =>
    invoke<void>('mcp_remove_server', { workspaceId, name }),
  setServer: (workspaceId: string | null, name: string, change: { enabled?: boolean; modes?: ToolMode[] }) =>
    invoke<void>('mcp_set_server', { workspaceId, name, enabled: change.enabled ?? null, modes: change.modes ?? null }),
  setTool: (
    workspaceId: string | null,
    server: string,
    tool: string,
    change: { enabled?: boolean; approval?: Approval },
  ) =>
    invoke<void>('mcp_set_tool', {
      workspaceId,
      server,
      tool,
      enabled: change.enabled ?? null,
      approval: change.approval ?? null,
    }),
  openConfig: (workspaceId: string | null) => invoke<string>('mcp_open_config', { workspaceId }),
  installEnola: () => invoke<string>('enola_install'),
  prepareSkills: (source: string) => invoke<StagedInstall>('skills_prepare_install', { source }),
  prepareRecommended: (id: string) => invoke<StagedInstall>('skills_prepare_recommended', { id }),
  setSkillModes: (name: string, modes: ToolMode[]) => invoke<void>('skills_set_modes', { name, modes }),
  confirmSkills: (token: string) => invoke<string[]>('skills_confirm_install', { token }),
  cancelSkills: (token: string) => invoke<void>('skills_cancel_install', { token }),
  setSkill: (workspaceId: string | null, name: string, enabled: boolean) =>
    invoke<void>('skills_set_enabled', { workspaceId, name, enabled }),
  removeSkill: (name: string) => invoke<boolean>('skills_remove', { name }),
  forChat: (workspaceId: string | null, mode: ToolMode) => invoke<ChatToolsView>('tools_for_chat', { workspaceId, mode }),
};

/** A Tauri command error as text. */
export function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Unknown error';
}
