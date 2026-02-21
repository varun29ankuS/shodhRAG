// TypeScript types matching Rust backend agent types

export type StepType =
  | 'Reasoning'
  | 'ToolCall'
  | 'RAGSearch'
  | 'LLMGeneration'
  | 'FinalSynthesis'
  | 'ErrorRecovery';

export interface AgentProgress {
  current_step: number;
  total_steps: number;
  step_type: StepType;
  message: string;
  percentage: number; // 0-100
  elapsed_ms: number;
}

export interface ExecutionStep {
  step_number: number;
  step_type: StepType;
  timestamp: number;
  duration_ms: number;
  input: string;
  output: string;
  tool_used: string | null;
  success: boolean;
}

export interface ExecutionResult {
  response: string;
  steps: ExecutionStep[];
  tools_used: string[];
  execution_time_ms: number;
  success: boolean;
  error: string | null;
  metadata: Record<string, any>;
}

export interface AgentInfo {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  tags: string[];
  icon?: string;
  color?: string;
  stats?: {
    totalRuns: number;
    avgResponseTime: number;
    successRate: number;
  };
}
