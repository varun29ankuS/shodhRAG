/**
 * What a Code or Creator answer did, read from the transcript the computer
 * saved with it (app/src/features/agent/reducer.ts, `TranscriptState`): the
 * files the agent changed, the commands it ran with how they ended and the
 * tail of their output, the steps still waiting for an approval when it was
 * saved, and Creator's finish summary.
 *
 * The tool names are the desktop's (crates/shodh-rag/src/harness/code_mode.rs):
 * `edit` and `write` change files (`CHANGE_TOOLS`), and every command runs
 * through Shodh's `run_command`, whose transcript step carries the run
 * (`command`, app/src/features/agent/commandRun.ts). Only finished changes
 * count as changed: a refused, denied or failed edit changed nothing.
 *
 * Read only. The phone shows this work; it never approves any of it.
 */

const CHANGE_TOOLS: readonly string[] = ['edit', 'write'];
const RUN_COMMAND = 'run_command';
const FINISH_CREATION = 'finish_creation';

export interface ChangedFile {
  path: string;
  /** Written whole at least once (`write`), or only edited. */
  written: boolean;
  /** How many finished changes it had. */
  changes: number;
}

export type CommandEnd = 'running' | 'done' | 'timed_out' | 'stopped' | 'killed' | 'refused' | 'failed';

export interface CommandRan {
  stepId: string;
  command: string;
  end: CommandEnd;
  exitCode: number | null;
  /** The output's tail as the computer kept it (stdout and stderr as they arrived). */
  output: string;
  /** Earlier output was dropped to keep the tail. */
  outputCut: boolean;
  /** What acted on it, in words (a limit, why it was refused). */
  note: string | null;
}

export interface WaitingStep {
  stepId: string;
  label: string;
}

export interface CodeWork {
  files: ChangedFile[];
  commands: CommandRan[];
  waiting: WaitingStep[];
  /** Creator's summary of what it built (`finish_creation`), when it finished. */
  finished: string | null;
}

export const NO_WORK: CodeWork = { files: [], commands: [], waiting: [], finished: null };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value.trim() !== '' ? value.trim() : null;
}

const ENDS: readonly CommandEnd[] = ['running', 'done', 'timed_out', 'stopped', 'killed', 'refused'];

/** The transcript's steps in the order the answer shows them: as placed among its blocks, then by start. */
function orderedSteps(transcript: Record<string, unknown>): Array<[string, Record<string, unknown>]> {
  const steps: Record<string, unknown> = isRecord(transcript.steps) ? transcript.steps : {};
  const order: string[] = [];
  if (Array.isArray(transcript.blocks)) {
    for (const block of transcript.blocks) {
      if (isRecord(block) && block.kind === 'step' && typeof block.stepId === 'string' && block.stepId in steps && !order.includes(block.stepId)) {
        order.push(block.stepId);
      }
    }
  }
  const rest = Object.keys(steps)
    .filter(id => !order.includes(id))
    .sort((a, b) => {
      const at = (id: string) => {
        const step = steps[id];
        return isRecord(step) && typeof step.startedAtMs === 'number' ? step.startedAtMs : 0;
      };
      return at(a) - at(b);
    });
  const out: Array<[string, Record<string, unknown>]> = [];
  for (const id of [...order, ...rest]) {
    const step = steps[id];
    if (isRecord(step)) out.push([id, step]);
  }
  return out;
}

function commandOf(stepId: string, step: Record<string, unknown>): CommandRan | null {
  const run = isRecord(step.command) ? step.command : null;
  const args = isRecord(step.args) ? step.args : {};
  const command = text(run?.command) ?? text(args.command);
  if (command === null) return null;
  const state = run && ENDS.includes(run.state as CommandEnd) ? (run.state as CommandEnd) : null;
  const end: CommandEnd = state ?? (step.status === 'done' ? 'done' : step.status === 'running' || step.status === 'awaiting_approval' ? 'running' : 'failed');
  return {
    stepId,
    command,
    end,
    exitCode: run && typeof run.exitCode === 'number' ? run.exitCode : null,
    output: run && typeof run.output === 'string' ? run.output : '',
    outputCut: run?.outputCut === true,
    note: text(run?.note) ?? (state === null && step.status === 'failed' ? text(step.summary) : null),
  };
}

/** The work of one saved answer; nothing for an answer saved without a transcript. */
export function codeWork(transcript: unknown): CodeWork {
  if (!isRecord(transcript)) return NO_WORK;
  const files = new Map<string, ChangedFile>();
  const commands: CommandRan[] = [];
  const waiting: WaitingStep[] = [];
  let finished: string | null = null;
  for (const [stepId, step] of orderedSteps(transcript)) {
    const tool = typeof step.tool === 'string' ? step.tool : '';
    if (step.status === 'awaiting_approval') waiting.push({ stepId, label: text(step.label) ?? 'A step' });
    if (CHANGE_TOOLS.includes(tool) && step.status === 'done') {
      const path = isRecord(step.args) ? text(step.args.path) : null;
      if (path !== null) {
        const known = files.get(path);
        files.set(path, { path, written: (known?.written ?? false) || tool === 'write', changes: (known?.changes ?? 0) + 1 });
      }
    }
    if (tool === RUN_COMMAND || isRecord(step.command)) {
      const ran = commandOf(stepId, step);
      if (ran) commands.push(ran);
    }
    if (tool === FINISH_CREATION && step.status === 'done' && isRecord(step.detail)) {
      finished = text(step.detail.summary) ?? finished;
    }
  }
  return { files: [...files.values()], commands, waiting, finished };
}

/** How a command ended, in words. */
export function commandEndWords(command: CommandRan): string {
  switch (command.end) {
    case 'done':
      return command.exitCode === null ? 'Finished' : command.exitCode === 0 ? 'Exit 0' : `Failed, exit ${command.exitCode}`;
    case 'running':
      return 'Running on your computer';
    case 'timed_out':
      return 'Timed out';
    case 'stopped':
      return 'Stopped';
    case 'killed':
      return 'Stopped by a limit';
    case 'refused':
      return 'Not run';
    case 'failed':
      return 'Failed';
  }
}

/** Whether the command ended well (done with exit 0, or no exit code reported). */
export function commandOk(command: CommandRan): boolean {
  return command.end === 'done' && (command.exitCode === null || command.exitCode === 0);
}
