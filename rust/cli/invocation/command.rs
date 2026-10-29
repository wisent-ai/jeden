//! Every top-level command `jeden` answers to, declared once and spelled by
//! its serde name on the command line. The parser, the dispatcher and the
//! properties that decide how a command's arguments are read come from this
//! declaration instead of separate lists of command words.

/// One top-level command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Command {
    #[default]
    Interactive,
    Version,
    Help,
    Doctor,
    Conformance,
    Run,
    Pursue,
    Hillclimb,
    Todo,
    Asks,
    Rpc,
    Headless,
    Acp,
    Relay,
    Import,
    Sessions,
    Copy,
    Show,
    Export,
    Artifacts,
    Artifact,
    Tools,
    Resume,
    Update,
    Config,
    Workspace,
    Contracts,
    Context,
    Roadmap,
    Capabilities,
    Completions,
    Token,
    Lease,
    ChatMemory,
    Stats,
    Gallery,
}

impl Command {
    /// The command `word` names, refused naming the word and every command.
    pub(crate) fn named(word: &str) -> Result<Self, String> {
        crate::vocabulary::named(word).map_err(|cause| format!("unknown command: {word} ({cause})"))
    }

    /// How the command is spelled on the command line.
    pub(crate) fn name(self) -> String {
        crate::vocabulary::spelling(self)
    }

    /// A command that works in a workspace: its directory is the workspace
    /// the operator adopted when `--cwd` does not name one.
    pub(crate) fn workspace_bound(self) -> bool {
        matches!(
            self,
            Command::Interactive
                | Command::Run
                | Command::Pursue
                | Command::Hillclimb
                | Command::Rpc
                | Command::Acp
        )
    }

    /// A command that needs a task (or, for pursue, a machine request).
    pub(crate) fn takes_task(self) -> bool {
        matches!(self, Command::Run | Command::Pursue)
    }

    /// A command whose own parser reads its `--options`: the invocation
    /// parser hands every option it does not know on as a positional.
    pub(crate) fn reads_own_options(self) -> bool {
        matches!(
            self,
            Command::Export
                | Command::Roadmap
                | Command::Token
                | Command::Stats
                | Command::Gallery
                | Command::Contracts
                | Command::Context
                | Command::Workspace
                | Command::Import
                | Command::Todo
                | Command::Asks
                | Command::Copy
                | Command::Hillclimb
        )
    }
}
