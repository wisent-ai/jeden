use std::env;
use std::path::PathBuf;
#[derive(Debug, Clone, Default)]
pub(crate) struct Args {
    pub(crate) command: String,
    pub(crate) cwd: PathBuf,
    pub(crate) cwd_explicit: bool,
    pub(crate) model: Option<String>,
    pub(crate) max_tokens: Option<u32>,
    pub(crate) max_steps: Option<u32>,
    pub(crate) allow_write: bool,
    pub(crate) allow_command: bool,
    pub(crate) yolo: bool,
    pub(crate) goal: Option<String>,
    pub(crate) model_only: bool,
    pub(crate) json: bool,
    pub(crate) resume_session: Option<PathBuf>,
    pub(crate) autonomous: bool,
    pub(crate) positionals: Vec<String>,
}

pub(crate) fn usage() -> String {
    concat!(
        "Usage:\n",
        "  jeden [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]\n",
        "  jeden --version | -V\n",
        "  jeden run \"task\" [--json] [--model-only] [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]\n",
        "  jeden pursue \"rough objective\" [--json] [--cwd path] [--model name] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]\n",
        "  jeden todo [list|add|pause|resume|cancel|continue] [--session id] [--revision n --reason text] [--json]\n",
        "  jeden rpc              serve newline-delimited JSON RPC on stdio\n",
        "  jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem> <identity-map.json> [revoked-serials.txt]\n",
        "  jeden acp              serve ACP on stdio\n",
        "  jeden sessions [limit]\n",
        "  jeden show <session-id-or-path>\n",
        "  jeden export <session-id-or-path> [output.json]\n",
        "  jeden artifacts <session-id-or-path>\n",
        "  jeden artifact <session-id-or-path> <name> [output]\n",
        "  jeden copy <text> | jeden copy - [--check] [--json] — hand an exact payload to the operator's clipboard, read back to confirm; --check asks later whether it is still there\n",
        "  jeden config [list|path|get <key>|set <key> <value>|reset <key>] [--json] [--cwd path]\n",
        "  jeden workspace [status|discover [path]|adopt <path>] [--json]\n",
        "  jeden contracts [render|status|install] [--omp|--file <path>] [--json] [--cwd path]\n",
        "  jeden doctor [--json] [--cwd path]\n",
        "  jeden conformance [--json] [--cwd path]\n",
        "  jeden probierz [args...] — run Probierz discovery, evidence, and gate commands for Jeden\n",
        "  jeden capabilities [--json] [--cwd path]\n",
        "  jeden completions <bash|zsh|fish>\n",
        "  jeden worktree [list|clear] [--dry-run] [--json] [--cwd path]\n",
        "  jeden token [--list] [--reveal] [--json] — print the agent Brama credential (redacted by default)\n",
        "  jeden stats [--json|--summary|--serve [--port N]] — usage/quota snapshot or local web dashboard\n",
        "  jeden gallery [--theme NAME|--all] [--color] — render TUI components across themes (dev tool)\n\n",
        "  jeden roadmap <list|show|add|drop|start|implemented|block|pass|status|depends|undepends|graph|acceptance|check|work> [args] [--json] [--cwd path]\n\n",
        "Slash commands:\n",
        "  /login [provider]      inspect entitlements-router login/reauth plan\n",
        "  /logout [provider]     show Weles-managed logout ownership\n",
        "  /settings              show auth and provider status\n",
        "  /setup                 guided first-run configuration\n",
        "  /model [name]          show or set model route\n",
        "  /mcp [list|tools|resources|prompts|notifications|test|reload|reconnect]\n",
        "  /marketplace [list|discover|installed|add|remove|install|uninstall|upgrade]\n",
        "  /plugins [list|enable|disable]\n",
        "  /approval [status|mode|set|reset]\n",
        "  /tools                 show available tools\n",
        "  /usage [show|reset]    show token/cost accounting\n",
        "  /browser [status|headless|visible]\n",
        "  /plan [on|off|status]  control plan mode\n",
        "  /goal [set|done|drop|auto on|off]  control goal mode; auto lets Oko's lifecycle model start/finish goals\n",
        "  /loop [on|off|status]  control continuation loop\n",
        "  /todo [list|add|pause|resume|cancel|continue]  manage retained session work\n",
        "  /roadmap              open the native roadmap picker\n",
        "  /memory [stats|view|enqueue|rebuild|clear]\n",
        "  /copy <text>           copy text to the clipboard and read it back to confirm\n",
        "  /collab [status|start|share|sync|stop]\n",
        "  /join <relay-file-url-or-path>\n",
        "  /leave\n",
        "  /dump [session]\n",
        "  /export [session] [--format json|text] [--output file]\n",
        "  /share [copy]\n",
        "  /omfg <complaint>\n",
        "  /tan <work>\n",
        "  /jobs\n",
        "  /changelog\n",
        "  /extensions\n",
        "  /reload-plugins\n",
        "  /rebuild              rebuild Jeden and resume this session\n",
        "  /retry\n",
        "  /btw <question>\n",
        "  /compact [focus]\n",
        "  /handoff [focus]\n",
        "  /clear|/new|/fresh\n",
        "  /fork | /branch <title> | /tree | /resume <session>\n",
        "  /rename <name> | /drop | /context | /move <dir>\n",
    )
    .to_string()
}

pub(crate) fn parse_args(argv: Vec<String>) -> Result<Args, String> {
    let mut rest = argv.into_iter();
    let first = rest.next();
    let mut command = first.unwrap_or_else(|| "interactive".to_string());
    if command == "--version" || command == "-V" {
        return Ok(Args {
            command: "version".into(),
            cwd: env::current_dir().unwrap_or_default(),
            ..Default::default()
        });
    }
    if command == "--help" || command == "-h" {
        return Ok(Args {
            command: "help".into(),
            cwd: env::current_dir().unwrap_or_default(),
            ..Default::default()
        });
    }
    if matches!(
        command.as_str(),
        "resume" | "recall_conversation" | "recall-conversation" | "search-sessions" | "probierz"
    ) {
        return Ok(Args {
            command,
            cwd: env::current_dir().map_err(|e| e.to_string())?,
            positionals: rest.collect(),
            ..Default::default()
        });
    }
    if command.starts_with("--") {
        rest = std::iter::once(command)
            .chain(rest)
            .collect::<Vec<_>>()
            .into_iter();
        command = "interactive".into();
    }
    let mut args = Args {
        command,
        cwd: env::current_dir().map_err(|e| e.to_string())?,
        ..Default::default()
    };
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--cwd" => {
                args.cwd = PathBuf::from(rest.next().ok_or("--cwd requires a value")?);
                args.cwd_explicit = true;
            }
            "--model" => args.model = Some(rest.next().ok_or("--model requires a value")?),
            "--max-tokens" => {
                args.max_tokens = Some(
                    rest.next()
                        .ok_or("--max-tokens requires a value")?
                        .parse()
                        .map_err(|_| "--max-tokens must be an integer")?,
                )
            }
            "--max-steps" => {
                args.max_steps = Some(
                    rest.next()
                        .ok_or("--max-steps requires a value")?
                        .parse()
                        .map_err(|_| "--max-steps must be an integer")?,
                )
            }
            "--allow-write" => args.allow_write = true,
            "--allow-command" => args.allow_command = true,
            "--resume-session" => {
                args.resume_session = Some(PathBuf::from(
                    rest.next().ok_or("--resume-session requires a path")?,
                ))
            }
            "--yolo" | "--auto-approve" => {
                args.yolo = true;
                args.allow_write = true;
                args.allow_command = true;
            }
            "--model-only" => args.model_only = true,
            "--json" => args.json = true,
            other
                if other.starts_with("--")
                    && (matches!(
                        args.command.as_str(),
                        "export"
                            | "roadmap"
                            | "worktree"
                            | "token"
                            | "stats"
                            | "gallery"
                            | "contracts"
                            | "workspace"
                            | "todo"
                            | "copy"
                    ) || (args.command == "run" && !args.positionals.is_empty())) =>
            {
                args.positionals.push(other.to_string())
            }
            other if other.starts_with("--") => return Err(format!("unknown option: {}", other)),
            other => args.positionals.push(other.to_string()),
        }
    }
    if matches!(args.command.as_str(), "run" | "pursue") && args.positionals.is_empty() {
        return Err(format!("{} requires a task", args.command));
    }
    if args.command == "interactive" && !args.positionals.is_empty() {
        return Err(format!(
            "unknown command: {}",
            args.positionals
                .first()
                .map(String::as_str)
                .unwrap_or_default()
        ));
    }
    Ok(args)
}
