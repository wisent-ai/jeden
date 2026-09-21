//! `jeden context`: the context advisor on the command line.
//!
//! `recommend` answers what to read for a task, `sources` says what each
//! source is and whether it answers right now, and `install` puts the same
//! advisor into an Omp session as a custom tool.

mod omp;


use serde_json::json;

use crate::context::advisor;
use crate::Args;

const USAGE: &str = concat!(
    "Usage:\n",
    "  jeden context recommend \"<task>\" [--limit n] [--source list] [--json] [--cwd path]\n",
    "  jeden context prompt \"<task>\" [--json] [--cwd path]\n",
    "  jeden context sources [--json] [--cwd path]\n",
    "  jeden context install [--omp|--file <path>] [--json]\n",
    "  jeden context installed [--omp|--file <path>] [--json]\n",
    "\n",
    "Sources: docs, ground-truth, memory, transcripts, or all.",
);
struct Options {
    query: String,
    limit: Option<usize>,
    sources: Option<String>,
}

/// Flags may appear before or after the task text; the task is whatever is
/// left, joined, so an unquoted sentence still works.
fn options(rest: &[String]) -> Result<Options, String> {
    let mut options = Options {
        query: String::new(),
        limit: None,
        sources: None,
    };
    let mut words: Vec<String> = Vec::new();
    let mut iter = rest.iter();
    while let Some(token) = iter.next() {
        match token.as_str() {
            "--limit" => {
                let value = iter.next().ok_or("--limit requires a number")?;
                options.limit = Some(
                    value
                        .parse()
                        .map_err(|_| format!("--limit must be a number, not {value}"))?,
                );
            }
            "--source" | "--sources" => {
                options.sources = Some(iter.next().ok_or("--source requires a list")?.clone());
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown context option: {other}\n{USAGE}"))
            }
            other => words.push(other.to_string()),
        }
    }
    options.query = words.join(" ").trim().to_string();
    Ok(options)
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
    let (verb, rest) = args
        .positionals
        .split_first()
        .map(|(verb, rest)| (verb.as_str(), rest))
        .unwrap_or(("help", &[]));
    match verb {
        "recommend" => recommend(args, rest),
        "prompt" => prompt(args, rest),
        "sources" => sources(args, rest),
        "install" => install(args, rest),
        "installed" | "status" => installed(args, rest),
        // A bare query is the common case: `jeden context "why does X fail"`.
        other if !other.starts_with("--") => {
            let mut all = vec![other.to_string()];
            all.extend_from_slice(rest);
            recommend(args, &all)
        }
        _ => Err(USAGE.into()),
    }
}

/// The block a turn would append for `task`, and nothing else: the one way to
/// see what the model will actually receive without running a turn.
fn prompt(args: &Args, rest: &[String]) -> Result<String, String> {
    let options = options(rest)?;
    if options.query.is_empty() {
        return Err(format!("context prompt requires a task\n{USAGE}"));
    }
    let config = crate::load_config(&args.cwd);
    let settings = advisor::settings(&args.cwd, &config);
    let block = advisor::advice_for_prompt(&args.cwd, &config, &options.query);
    if args.json {
        return Ok(serde_json::to_string_pretty(&json!({
            "query": options.query,
            "enabled": settings.enabled,
            "sources": settings.sources,
            "maxChars": settings.max_chars,
            "block": block,
        }))
        .map_err(|error| error.to_string())?
            + "\n");
    }
    match block {
        Some(block) => Ok(block),
        None if !settings.enabled => {
            Ok("The context advisor is off: context.advisor.enabled is false.\n".to_string())
        }
        None if settings.sources.is_empty() => {
            Ok("No context source is selected: context.advisor.sources resolved to nothing.\n"
                .to_string())
        }
        None => Ok("No source had anything to recommend for this task.\n".to_string()),
    }
}

fn recommend(args: &Args, rest: &[String]) -> Result<String, String> {
    let options = options(rest)?;
    if options.query.is_empty() {
        return Err(format!("context recommend requires a task\n{USAGE}"));
    }
    let settings = advisor::settings(&args.cwd, &crate::load_config(&args.cwd));
    let mut request = advisor::Request::from_settings(&options.query, &settings);
    if let Some(limit) = options.limit {
        request.limit = advisor::bounded_limit(limit);
    }
    if let Some(declared) = &options.sources {
        let unknown = advisor::unknown_sources(declared);
        if !unknown.is_empty() {
            return Err(format!(
                "unknown source(s): {}. Known sources: {}",
                unknown.join(", "),
                advisor::SOURCES.join(", ")
            ));
        }
        request.sources = advisor::parse_sources(declared);
    }
    if request.sources.is_empty() {
        return Err("no source selected: context.advisor.sources resolved to nothing".to_string());
    }
    let advice = advisor::recommend(&args.cwd, &crate::load_config(&args.cwd), &request);
    if args.json {
        return Ok(serde_json::to_string_pretty(&advice)
            .map_err(|error| error.to_string())?
            + "\n");
    }
    Ok(advisor::render_text(&advice))
}

fn sources(args: &Args, rest: &[String]) -> Result<String, String> {
    if !rest.is_empty() {
        return Err(USAGE.into());
    }
    let report = advisor::sources_report(&args.cwd, &crate::load_config(&args.cwd));
    if args.json {
        return Ok(serde_json::to_string_pretty(&report).map_err(|error| error.to_string())? + "\n");
    }
    let mut out = String::new();
    let selected = report
        .get("selected")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    out.push_str(&format!("Selected for every run: {selected}\n\n"));
    if let Some(rows) = report.get("sources").and_then(|value| value.as_array()) {
        for row in rows {
            let name = row
                .get("source")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let available = row
                .get("available")
                .and_then(|value| value.as_bool())
                .unwrap_or_default();
            let detail = row
                .get("detail")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            out.push_str(&format!(
                "- {name}: {} — {detail}\n",
                advisor::availability_word(available)
            ));
        }
    }
    Ok(out)
}

fn install(args: &Args, rest: &[String]) -> Result<String, String> {
    let target = omp::target(rest)?;
    let rendered = omp::rendered()?;
    let changed = omp::install(&target.file, &rendered)?;
    let path = target.file.display().to_string();
    if args.json {
        return Ok(serde_json::to_string_pretty(&json!({
            "target": target.name,
            "path": path,
            "tool": "context_recommend",
            "changed": changed,
        }))
        .map_err(|error| error.to_string())?
            + "\n");
    }
    Ok(if changed {
        format!("Installed the context_recommend tool into {path}\n")
    } else {
        format!("{path} already carries this binary's context_recommend tool\n")
    })
}

fn installed(args: &Args, rest: &[String]) -> Result<String, String> {
    let target = omp::target(rest)?;
    let rendered = omp::rendered()?;
    let state = omp::state(&target.file, &rendered);
    let path = target.file.display().to_string();
    if args.json {
        return Ok(serde_json::to_string_pretty(&json!({
            "target": target.name,
            "path": path,
            "tool": "context_recommend",
            "state": state,
        }))
        .map_err(|error| error.to_string())?
            + "\n");
    }
    let text = match state {
        "current" => format!("current: {path} carries this binary's context_recommend tool\n"),
        "stale" => format!(
            "stale: {path} carries a different context_recommend tool; run jeden context install --{}\n",
            target.name
        ),
        _ => format!(
            "absent: {path} carries no context_recommend tool; run jeden context install --{}\n",
            target.name
        ),
    };
    if state == "current" {
        Ok(text)
    } else {
        Err(text.trim_end().to_string())
    }
}
