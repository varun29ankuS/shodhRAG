//! Agent modes: how much of an answer's work runs without asking.
//!
//! A conversation answers in one mode, chosen by the user (a picker in the
//! composer; the default is in Settings). The agent can never change it: the
//! settings tools refuse it by name, no tool sets it, and a session's mode is
//! fixed when it starts (a change applies to the next answer, which starts a
//! session in the new mode).
//!
//! | Mode | Runs without asking | Still asks |
//! |---|---|---|
//! | [`AgentMode::Ask`] | reads, searches | every change, command and outward action |
//! | [`AgentMode::AcceptEdits`] | + edits inside the project or workspace | commands, anything leaving the computer |
//! | [`AgentMode::Plan`] | reads only: tools that change anything are not given to the model | — (the user approves the plan, then picks the mode it runs in) |
//! | [`AgentMode::Auto`] | + commands that match Shodh's rules ([`rules`]): ones that only inspect everywhere; builds and tests where commands are sandboxed, in a workspace the user trusts, or after the conversation's first was approved | every other command, anything leaving the computer |
//!
//! The floor no mode removes: destructive tools, tools that always confirm
//! ([`super::tools::HostTool::must_confirm_in`]: GitHub pushes, pull
//! requests, comments and merges, memories, downloads), and every write tool
//! that does not declare itself a local edit
//! ([`super::tools::HostTool::edits_locally`]) ask in every mode.
//!
//! Every decision is an exhaustive `match` on the mode (no wildcard arm), so a
//! new mode fails to compile until each decision says what it does.

pub mod rules;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use super::events::RiskTier;

/// How much of an answer's work runs without asking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentMode {
    /// Reads and searches run; every change asks (the default).
    #[default]
    Ask,
    /// Edits inside the project or workspace also run.
    AcceptEdits,
    /// Reads only: the agent investigates and writes a plan.
    Plan,
    /// Edits and the commands Shodh's rules recognise as low-risk also run.
    Auto,
}

impl AgentMode {
    /// Every mode, in the order the picker shows and Shift+Tab cycles them.
    pub const ALL: [AgentMode; 4] = [
        AgentMode::Ask,
        AgentMode::AcceptEdits,
        AgentMode::Plan,
        AgentMode::Auto,
    ];

    /// The name the UI and the audit log use.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentMode::Ask => "ask",
            AgentMode::AcceptEdits => "acceptEdits",
            AgentMode::Plan => "plan",
            AgentMode::Auto => "auto",
        }
    }

    /// The mode named `name` ([`AgentMode::as_str`]).
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == name)
    }

    /// The mode in plain words ("Accept edits mode").
    pub fn label(self) -> &'static str {
        match self {
            AgentMode::Ask => "Ask mode",
            AgentMode::AcceptEdits => "Accept edits mode",
            AgentMode::Plan => "Plan mode",
            AgentMode::Auto => "Auto mode",
        }
    }

    /// Whether the model may only read: tools that change anything are not
    /// given to it.
    pub fn reads_only(self) -> bool {
        match self {
            AgentMode::Plan => true,
            AgentMode::Ask | AgentMode::AcceptEdits | AgentMode::Auto => false,
        }
    }

    /// What the system prompt tells the model about the mode it answers in.
    pub fn prompt_note(self) -> &'static str {
        match self {
            AgentMode::Ask => {
                "Mode: Ask. Reads and searches run by themselves; every change, command and \
                 action outside this computer waits for the user's approval."
            }
            AgentMode::AcceptEdits => {
                "Mode: Accept edits. Edits inside the project or workspace run without asking; \
                 commands and anything that leaves this computer still wait for the user's \
                 approval."
            }
            AgentMode::Plan => PLAN_NOTE,
            AgentMode::Auto => {
                "Mode: Auto. Edits and commands that only read (git status, diff and log, ls, \
                 cat, grep, a linter that changes nothing) run without asking. Builds and tests \
                 run the project's own code: they run without asking only where commands are \
                 sandboxed or in a workspace the user trusts, and otherwise the first one in a \
                 conversation asks. Any other command, and anything that leaves this computer, \
                 waits for the user's approval. Run one plain command per call: chained, piped \
                 or redirected commands always ask."
            }
        }
    }
}

/// What a Plan mode answer is told: investigate, then write the plan.
const PLAN_NOTE: &str = "Mode: Plan. You can only read and search: tools that change anything \
     are not available in this mode, and you must not ask for them. Investigate what the task \
     needs, then answer with a plan: a short summary, then numbered steps, each naming what \
     changes (files, commands, data) and how to check it, then the risks and open questions. \
     Do not make the changes. The user approves the plan and chooses the mode it runs in; you \
     cannot change the mode. If the task needs no changes, just answer it.";

/// What kind of action a host tool call is, for the mode's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolClass {
    /// Reads and searches.
    Read,
    /// A tool the user set to run without asking (an MCP tool set to
    /// "auto"): it reports the read tier but can change things.
    UserAllowed,
    /// A change that stays on this computer, inside the user's own data or
    /// workspace ([`super::tools::HostTool::edits_locally`]).
    LocalEdit,
    /// Any other change: it may leave the computer or be hard to take back.
    Write,
    /// Hard to undo.
    Destructive,
}

impl ToolClass {
    /// The class of a tool with `tier` that edits locally (`local_edit`) or
    /// changes files without asking (`changes_unasked`).
    pub fn of(tier: RiskTier, local_edit: bool, changes_unasked: bool) -> Self {
        match tier {
            RiskTier::Read if changes_unasked => ToolClass::UserAllowed,
            RiskTier::Read => ToolClass::Read,
            RiskTier::Write if local_edit => ToolClass::LocalEdit,
            RiskTier::Write => ToolClass::Write,
            RiskTier::Destructive => ToolClass::Destructive,
        }
    }
}

/// What happens to a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Runs without asking (a tool's own `must_confirm` can still ask).
    Run,
    /// Waits for the user.
    Ask,
    /// Not given to the model in this mode, and refused if called.
    Hidden,
}

/// The mode's decision for a host tool of `class`.
pub fn decide(mode: AgentMode, class: ToolClass) -> Gate {
    match mode {
        AgentMode::Ask => match class {
            ToolClass::Read | ToolClass::UserAllowed => Gate::Run,
            ToolClass::LocalEdit | ToolClass::Write | ToolClass::Destructive => Gate::Ask,
        },
        AgentMode::AcceptEdits | AgentMode::Auto => match class {
            ToolClass::Read | ToolClass::UserAllowed | ToolClass::LocalEdit => Gate::Run,
            ToolClass::Write | ToolClass::Destructive => Gate::Ask,
        },
        AgentMode::Plan => match class {
            ToolClass::Read => Gate::Run,
            ToolClass::UserAllowed
            | ToolClass::LocalEdit
            | ToolClass::Write
            | ToolClass::Destructive => Gate::Hidden,
        },
    }
}

/// Whether a Code session's edits and writes (in files that cannot run
/// programs, while a branch can undo them) run without asking.
pub fn code_edits(mode: AgentMode) -> Gate {
    match mode {
        AgentMode::Ask => Gate::Ask,
        AgentMode::AcceptEdits | AgentMode::Auto => Gate::Run,
        AgentMode::Plan => Gate::Hidden,
    }
}

/// How a Code session's commands are decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandGate {
    /// Every command asks.
    Ask,
    /// A command Shodh's rules or the workspace's list recognise runs
    /// without asking (with the review and running checks of Code mode).
    Rules,
    /// No command tool: the mode reads only.
    Hidden,
}

/// The mode's decision for commands.
pub fn code_commands(mode: AgentMode) -> CommandGate {
    match mode {
        AgentMode::Ask | AgentMode::AcceptEdits => CommandGate::Ask,
        AgentMode::Auto => CommandGate::Rules,
        AgentMode::Plan => CommandGate::Hidden,
    }
}

/// Why a call ran without asking, for the audit log ("Ran without asking ·
/// Auto mode").
pub fn audit_reason(mode: AgentMode, what: &str) -> String {
    format!("{}: {what}", mode.label())
}
