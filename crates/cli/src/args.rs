//! The `ax` command line: one clap definition and no behaviour.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::config;

#[derive(Parser)]
#[command(
    name = "ax",
    version,
    about = "A fast, lightweight AI agent for the terminal"
)]
pub(crate) struct Cli {
    /// Runtime isolation: off, workspace, strict. Never falls back to host.
    #[arg(long, global = true)]
    pub(crate) sandbox: Option<sandbox::SandboxMode>,
    /// Update this AX executable from the latest GitHub Release.
    #[arg(long)]
    pub(crate) update: bool,
    /// Provider, when chosen explicitly. Takes a provider id from the catalog
    /// (`deepseek`, `xiaomi-token-plan-cn`, ...) or one of the aliases
    /// `deepseek` | `openai` | `codex` | `compatible`. Naming the id matters
    /// when a vendor is listed more than once — Xiaomi and its token-plan
    /// regions share model ids but not endpoints. Otherwise AX resolves the
    /// selection from the persisted config, then local credential detection.
    #[arg(long, global = true)]
    pub(crate) provider: Option<String>,
    #[arg(long, global = true)]
    pub(crate) model: Option<String>,
    /// Override the model's thinking depth: low | medium | high | xhigh | max.
    #[arg(long, global = true)]
    pub(crate) reasoning_effort: Option<String>,
    #[arg(long, global = true)]
    pub(crate) codex_auth: Option<PathBuf>,
    /// Override the active model's context-window token capacity.
    #[arg(long, global = true)]
    pub(crate) context_window: Option<NonZeroUsize>,
    /// Override storage (default: installation .ax/projects/<project-key>).
    #[arg(long, global = true)]
    pub(crate) data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(crate) skills_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(crate) mcp_config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(crate) allow_dangerous: bool,
    /// Maximum model steps per turn; 0 (default) means unlimited.
    #[arg(long, global = true, default_value_t = 0)]
    pub(crate) max_steps: usize,
    /// Maximum tool calls per turn; 0 (default) means unlimited.
    #[arg(long, global = true, default_value_t = 0)]
    pub(crate) max_tool_calls: usize,
    /// Turn timeout in seconds; 0 (default) means unlimited.
    #[arg(long, global = true, default_value_t = 0)]
    pub(crate) turn_timeout_secs: u64,
    /// Timeout for each isolated child; does not end the controller. 0 is unlimited.
    #[arg(long, global = true, default_value_t = 0)]
    pub(crate) child_timeout_secs: u64,
    /// Tool timeout in seconds; 0 (default) means unlimited.
    #[arg(long, global = true, default_value_t = 0)]
    pub(crate) tool_timeout_secs: u64,
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Show or persist optional subagent settings; active agents reload next turn.
    Settings {
        #[arg(long)]
        max_concurrent: Option<usize>,
        #[arg(long)]
        max_depth: Option<usize>,
        /// Global defaults or overrides for the current project.
        #[arg(long, default_value = "global", value_parser = ["global", "project"])]
        scope: String,
        /// Reset global defaults, or inherit global values in this project.
        #[arg(long, conflicts_with_all = ["max_concurrent", "max_depth"])]
        reset: bool,
    },
    /// Show or change memory, custom instructions and the writing-style folder.
    Personalize {
        /// Master memory switch: retrieval, `remember` declarations and the memory tool.
        #[arg(long, action = clap::ArgAction::Set)]
        memory: Option<bool>,
        /// Allow tool-driven memory creation and automatic Experience learning.
        #[arg(long, action = clap::ArgAction::Set)]
        tool_memory: Option<bool>,
        #[arg(long, action = clap::ArgAction::Set)]
        writing: Option<bool>,
        /// Folder of the user's own documents used as a writing-style reference.
        #[arg(long, conflicts_with = "clear_writing_folder")]
        writing_folder: Option<std::path::PathBuf>,
        #[arg(long)]
        clear_writing_folder: bool,
        /// Replace the global custom instructions with this file's text; empty clears them.
        #[arg(long)]
        instructions_file: Option<std::path::PathBuf>,
        /// Reject a save if custom instructions no longer match this baseline.
        #[arg(long, requires = "instructions_file")]
        expected_instructions_file: Option<std::path::PathBuf>,
        /// Delete all remembered facts on this installation. Raw history is kept.
        #[arg(long)]
        clear_memories: bool,
    },
    /// Show or select the agent environment and Crew terminal shell.
    Environment {
        #[arg(value_enum)]
        environment: Option<config::AgentEnvironment>,
        #[arg(long, value_enum)]
        terminal_shell: Option<config::TerminalShell>,
    },
    /// Manage AX provider credentials.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Import a validated Skill directory into AX.
    Skill {
        #[command(subcommand)]
        command: CapabilityCommand,
    },
    /// Import MCP server configuration without launching servers.
    Mcp {
        #[command(subcommand)]
        command: CapabilityCommand,
    },
    /// Manage scoped Skills, MCP servers and named Agents through one registry.
    Capabilities {
        kind: String,
        #[arg(default_value = "list")]
        action: String,
        name: Option<String>,
        #[arg(long, default_value = "project")]
        scope: String,
        #[arg(long)]
        source: Option<PathBuf>,
    },
    /// Run the Agent Client Protocol v1 adapter on stdio.
    Acp,
    /// Pair or connect this AX instance to an AX Crew gateway.
    Crew {
        #[command(subcommand)]
        command: CrewCommand,
    },
    /// Export portable user data to a new .axpack archive.
    Export {
        path: PathBuf,
        #[arg(long)]
        memory: bool,
        #[arg(long)]
        sessions: bool,
    },
    /// Validate and merge a .axpack archive.
    Import {
        path: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Run one persisted task and exit.
    Run {
        prompt: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long, requires = "session", conflicts_with = "cancel_goal")]
        resume_goal: Option<String>,
        #[arg(long, requires = "session")]
        cancel_goal: Option<String>,
    },
    /// Run independent tasks with bounded concurrency.
    Agents {
        #[arg(required = true, num_args = 1..)]
        prompts: Vec<String>,
        #[arg(short, long, default_value_t = 4)]
        concurrency: usize,
    },
    /// Start the inline terminal UI while preserving native scrollback.
    Tui,
}
#[derive(Subcommand)]
pub(crate) enum CapabilityCommand {
    /// Import from a local directory (Skill) or TOML/JSON file (MCP).
    Import {
        path: PathBuf,
        /// Install globally instead of into the current project.
        #[arg(long)]
        global: bool,
    },
}
#[derive(Subcommand)]
pub(crate) enum AuthCommand {
    /// Remove a provider from AX, including automatic environment credential fallback.
    Remove { provider: String },
    /// Sign in through the system browser.
    Login {
        provider: String,
        /// `WorkBuddy` login region (intl or cn). Defaults to the provider's region.
        #[arg(long, value_enum)]
        region: Option<WorkBuddyLoginRegion>,
    },
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum WorkBuddyLoginRegion {
    Intl,
    Cn,
}
#[derive(Subcommand)]
pub(crate) enum CrewCommand {
    /// Run an opt-in distributed AX worker using a local JSON configuration.
    Worker { config: PathBuf },
    /// Pair this machine using a one-time code issued by Crew.
    Pair {
        code: String,
        #[arg(long, default_value = "http://127.0.0.1:8765")]
        gateway: String,
    },
    /// Maintain an outbound authenticated WebSocket connection to Crew.
    Connect { gateway: String },
}

#[cfg(test)]
mod goal_argument_tests {
    use clap::Parser;

    use super::*;
    use crate::runtime::execution_budget;

    #[test]
    fn child_timeout_is_separate_from_controller_turn_timeout() {
        let cli = Cli::try_parse_from([
            "ax",
            "--child-timeout-secs",
            "2",
            "--turn-timeout-secs",
            "60",
            "run",
            "goal",
        ])
        .unwrap();
        assert_eq!(cli.child_timeout_secs, 2);
        assert_eq!(execution_budget(&cli).turn_timeout_secs, 60);
    }

    #[test]
    fn resume_requires_session_and_explicit_goal() {
        assert!(Cli::try_parse_from(["ax", "run", "--resume-goal", "goal-1", "continue"]).is_err());
        let cli = Cli::try_parse_from([
            "ax",
            "run",
            "--session",
            "session-1",
            "--resume-goal",
            "goal-1",
            "continue",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Command::Run {
            session: Some(session), resume_goal: Some(goal), cancel_goal: None, ..
        }) if session == "session-1" && goal == "goal-1"));
        assert!(
            Cli::try_parse_from([
                "ax",
                "run",
                "--session",
                "session-1",
                "--resume-goal",
                "goal-1",
                "--cancel-goal",
                "goal-1",
                "continue"
            ])
            .is_err()
        );
    }
}
