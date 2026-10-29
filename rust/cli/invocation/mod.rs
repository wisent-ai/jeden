mod command;
pub(crate) mod refusal;

pub(crate) use command::Command;
use std::env;
use std::path::PathBuf;
#[derive(Debug, Clone, Default)]
pub(crate) struct Args {
    pub(crate) command: Command,
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
    pub(crate) pursuit_request: Option<crate::autonomy::requests::Mode>,
    pub(crate) positionals: Vec<String>,
}

pub(crate) fn usage() -> String {
    concat!(
        "Usage:\n",
        "  jeden [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo] [--max-steps n]\n",
        "  jeden --version | -V\n",
        "  jeden run \"task\" [--json] [--model-only] [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo] [--max-steps n]\n",
        "  jeden pursue \"rough objective\" [--json] [--cwd path] [--model name] [--allow-write] [--allow-command] [--yolo] [--max-steps n]\n",
        "  jeden pursue --request-file JSON [--allow-write] [--allow-command] [--json]\n",
        "  jeden pursue --status REQUEST_ID [--json]\n",
        "  jeden pursue --state REQUEST_ID [--json]\n",
        "  jeden pursue --resume-run REQUEST_ID [--json]\n",
        "  jeden hillclimb [focus] [--scope repository|organization [--only repo]...] [--rounds n] [--ledger path] --allow-write --allow-command [--json] [--cwd path] [--model name] — improve a repository or every repository of an organization round after round: choose the next improvement, pursue it, record it, choose again\n",
        "  jeden hillclimb --status [--ledger path] [--json] [--cwd path] — read recorded rounds without starting a climb or granting writes\n",
        "  jeden todo [list|add|pause|resume|cancel|defect|answer|continue] [--session id] [--revision n --reason text|--text answer] [--json]\n",
        "  jeden asks [list|show|answer] [ask-id] [--text answer] [--json] — everything Jeden asked of you, in every session, and what you answered\n",
        "  jeden rpc              serve newline-delimited JSON RPC on stdio\n",
        "  jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem> <identity-map.json> <limits.json> [revoked-serials.txt]\n",
        "  jeden acp              serve ACP on stdio\n",
        "  jeden relay serve <addr>  run the encrypted collaboration-room relay on <addr>\n",
        "  jeden sessions [limit] [--json]\n",
        "  jeden sessions search <query> [limit] [--json] — the newest event of each session whose payload holds the query\n",
        "  jeden import <path>... [--refresh] [--json] — preserve sessions of another harness in native ledgers without executing work\n",
        "  jeden show <session-id-or-path> [--json]\n",
        "  jeden export <session-id-or-path> <output> [--html|--markdown] — write a session to a file\n",
        "  jeden artifacts <session-id-or-path> [--json]\n",
        "  jeden artifact <session-id-or-path> <name> [output]\n",
        "  jeden copy <text> | jeden copy - [--check] [--json] — hand an exact payload to the operator's clipboard, read back to confirm; --check asks later whether it is still there\n",
        "  jeden config [list|path|get <key>|set <key> <value>|reset <key>|unset <key>] [--json] [--cwd path]\n",
        "  jeden workspace [status|discover [path]|adopt <path>|forget] [--json]\n",
        "  jeden contracts [render|status|install|uninstall] [--file <path>] [--json] [--cwd path]\n",
        "  jeden context [recommend] \"<task>\" [--limit n] [--source list] [--json] [--cwd path]\n",
        "  jeden context sources [--json] [--cwd path] — what each context source is and whether it answers\n",
        "  jeden doctor [--json] [--cwd path]\n",
        "  jeden conformance [--json] [--cwd path]\n",
        "  jeden capabilities [--json] [--cwd path]\n",
        "  jeden completions <bash|zsh|fish>\n",
        "  jeden token [status|list|add LABEL --endpoint URL|remove ID] [--json] — manage private model credentials; add reads the key from stdin\n",
        "  jeden stats [--json|--summary|--serve [--port N]] — usage/quota snapshot or local web dashboard\n",
        "  jeden gallery [--theme NAME|--all] [--color] — render TUI components across themes (dev tool)\n\n",
        "  jeden roadmap <list|show|add|edit|drop|start|implemented|block|unblock|pass|status|depends|undepends|graph|acceptance|check|work> [args] [--json] [--cwd path]\n\n",
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
        "  /goal [set|done|drop|auto on|off]  control goal mode; auto lets the lifecycle model (Brama alias JEDEN_LIFECYCLE_MODEL_ALIAS) start/finish goals\n",
        "  /loop [on|off|status]  control continuation loop\n",
        "  /hillclimb             what the hill-climb of this workspace has chosen and how each round ended\n",
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
        "  /new\n",
        "  /fork | /branch <title> | /tree | /resume <session>\n",
        "  /rename <name> | /drop | /context | /move <dir>\n",
    )
    .to_string()
}

/// The usage lines that name one subcommand, or the whole usage when none
/// does: what `jeden <subcommand> --help` prints.
pub(crate) fn command_usage(command: Option<&str>) -> String {
    let whole = usage();
    let Some(command) = command else {
        return whole;
    };
    let prefix = format!("  jeden {command}");
    let lines: Vec<&str> = whole
        .lines()
        .filter(|line| *line == prefix || line.starts_with(&format!("{prefix} ")))
        .collect();
    if lines.is_empty() {
        whole
    } else {
        format!("Usage:\n{}\n", lines.join("\n"))
    }
}

/// The command a whole-line flag stands for: `--version`/`-V` and
/// `--help`/`-h`, as the usage spells them.
fn standalone_flag(word: &str) -> Option<Command> {
    match word {
        "--version" => Some(Command::Version),
        "-V" => Some(Command::Version),
        "--help" => Some(Command::Help),
        "-h" => Some(Command::Help),
        _ => None,
    }
}

/// How a pursuit mode is built from its option's value.
type PursuitMode = fn(String) -> crate::autonomy::requests::Mode;

/// The pursuit mode a `jeden pursue` option selects, built from its value.
fn pursuit_mode(flag: &str) -> Option<PursuitMode> {
    use crate::autonomy::requests::Mode;
    let mode: PursuitMode = match flag {
        "--request-file" => |value| Mode::Submit(PathBuf::from(value)),
        "--status" => Mode::Status,
        "--state" => Mode::State,
        "--resume-run" => Mode::Resume,
        _ => return None,
    };
    Some(mode)
}

/// The directory the command runs in until `--cwd` names another.
fn current_dir() -> Result<PathBuf, String> {
    env::current_dir().map_err(|error| format!("the current directory cannot be read: {error}"))
}

pub(crate) fn parse_args(argv: Vec<String>) -> Result<Args, String> {
    let mut rest = argv.into_iter().peekable();
    let first = rest.peek().cloned();
    let command = match first.as_deref() {
        None => Command::Interactive,
        Some(word) => match standalone_flag(word) {
            Some(command) => {
                return Ok(Args {
                    command,
                    cwd: current_dir()?,
                    ..Default::default()
                })
            }
            None if word.starts_with("--") => Command::Interactive,
            None => {
                rest.next();
                Command::named(word)?
            }
        },
    };
    // `--help` or `-h` after a subcommand answers with that subcommand's
    // usage lines and never runs it, before any option reads its value.
    let rest_words: Vec<String> = rest.collect();
    if command != Command::Interactive
        && rest_words
            .iter()
            .any(|word| standalone_flag(word) == Some(Command::Help))
    {
        return Ok(Args {
            command: Command::Help,
            cwd: current_dir()?,
            positionals: vec![command.name()],
            ..Default::default()
        });
    }
    if command == Command::Resume {
        return Ok(Args {
            command,
            cwd: current_dir()?,
            positionals: rest_words,
            ..Default::default()
        });
    }
    let mut rest = rest_words.into_iter();
    let mut args = Args {
        command,
        cwd: current_dir()?,
        ..Default::default()
    };
    while let Some(arg) = rest.next() {
        let pursuit = pursuit_mode(&arg).filter(|_mode| args.command == Command::Pursue);
        if let Some(mode) = pursuit {
            if args.pursuit_request.is_some() {
                return Err(
                    "pursue request, status, state and resume modes are mutually exclusive".into(),
                );
            }
            let value = rest
                .next()
                .ok_or_else(|| format!("{arg} requires a value"))?;
            args.pursuit_request = Some(mode(value));
            continue;
        }
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
                        .map_err(|error| format!("--max-tokens must be an integer: {error}"))?,
                )
            }
            "--max-steps" => {
                args.max_steps = Some(
                    rest.next()
                        .ok_or("--max-steps requires a value")?
                        .parse()
                        .map_err(|error| format!("--max-steps must be an integer: {error}"))?,
                )
            }
            "--allow-write" => args.allow_write = true,
            "--allow-command" => args.allow_command = true,
            "--resume-session" => {
                args.resume_session = Some(PathBuf::from(
                    rest.next().ok_or("--resume-session requires a path")?,
                ))
            }
            "--yolo" => {
                args.yolo = true;
                args.allow_write = true;
                args.allow_command = true;
            }
            "--model-only" => args.model_only = true,
            "--json" => args.json = true,
            other
                if other.starts_with("--")
                    && (args.command.reads_own_options()
                        || (args.command == Command::Run && !args.positionals.is_empty())) =>
            {
                args.positionals.push(other.to_string())
            }
            other if other.starts_with("--") => return Err(format!("unknown option: {}", other)),
            other => args.positionals.push(other.to_string()),
        }
    }
    if args.pursuit_request.is_some() && !args.positionals.is_empty() {
        return Err("a machine pursuit request cannot also contain a positional objective".into());
    }
    if args.command.takes_task() && args.positionals.is_empty() && args.pursuit_request.is_none() {
        return Err(format!("{} requires a task", args.command.name()));
    }
    if args.command == Command::Interactive {
        if let Some(first) = args.positionals.first() {
            return Err(format!("unknown command: {first}"));
        }
    }
    Ok(args)
}
