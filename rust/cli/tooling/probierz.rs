use crate::{load_config, Args};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn source_checkout() -> Option<PathBuf> {
    let configured = env::var_os("PROBIERZ_ROOT").map(PathBuf::from);
    let sibling = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|parent| parent.join("probierz"));

    configured
        .into_iter()
        .chain(sibling)
        .find(|root| root.join("agent/cli.mjs").is_file())
}

fn apply_jeden_environment(command: &mut Command, args: &Args) -> Result<(), String> {
    if env::var_os("TUI_CMD").is_none() {
        let executable = env::current_exe()
            .map_err(|error| format!("resolve the current Jeden executable: {error}"))?;
        command.env("TUI_CMD", executable);
    }
    if env::var_os("JEDEN_MODEL").is_none() {
        let config = load_config(&args.cwd);
        if let Some(model) = args.model.as_ref().or(config.model.as_ref()) {
            command.env("JEDEN_MODEL", model);
        }
    }
    Ok(())
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
    let mut command = if let Some(root) = source_checkout() {
        let mut command = Command::new("node");
        command.arg(root.join("agent/cli.mjs")).current_dir(root);
        command
    } else {
        Command::new("probierz")
    };

    apply_jeden_environment(&mut command, args)?;
    if args.positionals.is_empty() {
        command.args(["status", "jeden", "--text"]);
    } else {
        command.args(&args.positionals);
    }

    let status = command.status().map_err(|error| {
        format!(
            "launch Probierz: {error}; set PROBIERZ_ROOT to a Probierz source checkout or install its CLI"
        )
    })?;
    if status.success() {
        Ok(String::new())
    } else {
        Err(format!("Probierz exited with {status}"))
    }
}
