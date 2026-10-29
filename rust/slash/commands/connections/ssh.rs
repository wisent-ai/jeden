use crate::cli::config::{merged_config_value, read_config_value, user_config_value};
use crate::slash::common::{
    project_config_for_edit, project_config_path, split_args, write_json_value,
};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};
use serde_json::{Map, Value};

fn ssh_hosts_from(config: &Value) -> Option<&Map<String, Value>> {
    config
        .get("sshHosts")
        .and_then(Value::as_object)
        .or_else(|| {
            config
                .get("ssh")
                .and_then(|ssh| ssh.get("hosts"))
                .and_then(Value::as_object)
        })
        .or_else(|| config.get("ssh").and_then(Value::as_object))
}

fn ssh_host_value(target: &str, options: &[String]) -> Option<Value> {
    if options.is_empty() {
        return Some(Value::String(target.to_string()));
    }
    let mut host = Map::new();
    host.insert("host".into(), Value::String(target.to_string()));
    for option in options {
        let (key, value) = option.split_once('=')?;
        if key.is_empty() {
            return None;
        }
        host.insert(key.to_string(), Value::String(value.to_string()));
    }
    Some(Value::Object(host))
}

/// What a configured host entry connects to: the entry itself when it is a
/// target, or its `host` field; an entry with neither says so.
fn host_target(value: &Value) -> String {
    match value
        .as_str()
        .or_else(|| value.get("host").and_then(Value::as_str))
    {
        Some(target) => target.to_string(),
        None => format!("no target recorded in {value}"),
    }
}

pub(crate) fn ssh_picker(context: &SlashContext<'_>) -> PickerSpec {
    let user = user_config_value();
    let project = read_config_value(&project_config_path(context.cwd));
    let effective = merged_config_value(context.cwd);
    let user_hosts = ssh_hosts_from(&user);
    let project_hosts = ssh_hosts_from(&project);
    let mut items = vec![PickerItem::action("Show configured SSH hosts", "/ssh list")
        .detail("Read effective user and project host aliases")
        .badge("status")];
    if let Some(hosts) = ssh_hosts_from(&effective) {
        for (name, value) in std::collections::BTreeMap::from_iter(hosts.iter()) {
            let project_owned = project_hosts.is_some_and(|hosts| hosts.contains_key(name));
            let scope = if project_owned {
                "project"
            } else if user_hosts.is_some_and(|hosts| hosts.contains_key(name)) {
                "user"
            } else {
                "merged"
            };
            let target = host_target(value);
            if project_owned {
                // The name goes into the command quoted as a JSON string, so a
                // name with spaces or quotes still names this host.
                let quoted = match serde_json::to_string(name) {
                    Ok(quoted) => quoted,
                    Err(error) => {
                        let mut unquotable = PickerItem::action(format!("Remove {name}"), "")
                            .detail(format!(
                                "its name cannot be quoted for the command: {error}"
                            ))
                            .disabled(true);
                        unquotable.command = None;
                        items.push(unquotable);
                        continue;
                    }
                };
                items.push(
                    PickerItem::action(format!("Remove {name}"), format!("/ssh remove {quoted}"))
                        .detail(format!("{target}; stored in project config"))
                        .badge("DESTRUCTIVE"),
                );
            } else {
                let mut inherited = PickerItem::action(format!("{name}: {target}"), "")
                    .detail(format!(
                        "Inherited from {scope} config; edit that source to remove it"
                    ))
                    .badge(scope)
                    .disabled(true);
                inherited.command = None;
                items.push(inherited);
            }
        }
    }
    let add = PickerItem::action("Add an SSH host", "/ssh add ")
        .detail("Edit host name, target, and optional key=value fields before submitting")
        .badge("INPUT")
        .prefill();
    items.push(add);
    PickerSpec::new("SSH hosts", items)
}

const USAGE: &str = "Usage: /ssh list | add <name> <target> [key=value ...] | remove <name> | help";

/// `/ssh` with no word lists the hosts, as `/ssh list` does.
pub(crate) fn handle_ssh(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let words: Vec<&str> = argv.iter().map(String::as_str).collect();
    let project_file = project_config_path(context.cwd);
    match words.as_slice() {
        [] | ["list"] => {
            let config = merged_config_value(context.cwd);
            let hosts = match ssh_hosts_from(&config) {
                Some(hosts) if !hosts.is_empty() => hosts,
                _none => {
                    return Ok("No SSH hosts configured in ~/.jeden/config.json or <cwd>/.jeden/config.json (sshHosts).".into())
                }
            };
            let mut lines = Vec::new();
            for (host, value) in std::collections::BTreeMap::from_iter(hosts.iter()) {
                let rendered = match value.as_str() {
                    Some(text) => text.to_string(),
                    None => serde_json::to_string(value).map_err(|error| error.to_string())?,
                };
                lines.push(format!("{host}\t{rendered}"));
            }
            Ok(lines.join("\n"))
        }
        ["help"] => Ok("Usage: /ssh list | add <name> <target> [key=value ...] | remove <name>. Hosts are stored in <cwd>/.jeden/config.json under sshHosts.".into()),
        ["add", name, target, options @ ..] => {
            let options: Vec<String> = options.iter().map(|option| (*option).to_string()).collect();
            let value = ssh_host_value(target, &options)
                .ok_or_else(|| "Usage: /ssh add <name> <target> [key=value ...]".to_string())?;
            let mut project = project_config_for_edit(context.cwd)?;
            let object = project.as_object_mut().ok_or_else(|| {
                format!("{} does not hold a JSON object, so no host is added", project_file.display())
            })?;
            let hosts = object
                .entry("sshHosts")
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .ok_or_else(|| {
                    format!(
                        "sshHosts in {} is not an object; it is left as it is and no host is added",
                        project_file.display()
                    )
                })?;
            hosts.insert((*name).to_string(), value);
            write_json_value(&project_file, &project)?;
            Ok(format!("Added SSH host {name} to {}.", project_file.display()))
        }
        ["add", ..] => Err("Usage: /ssh add <name> <target> [key=value ...]".into()),
        ["remove", name] => {
            let effective = merged_config_value(context.cwd);
            let mut project = project_config_for_edit(context.cwd)?;
            let Some(hosts) = project.get_mut("sshHosts").and_then(Value::as_object_mut).filter(|hosts| hosts.contains_key(*name)) else {
                if ssh_hosts_from(&effective).is_some_and(|hosts| hosts.contains_key(*name)) {
                    return Err(format!("SSH host {name} is not in <cwd>/.jeden/config.json. Remove it from ~/.jeden/config.json or the config file that defines it."));
                }
                return Err(format!("SSH host not found: {name}"));
            };
            hosts.remove(*name);
            write_json_value(&project_file, &project)?;
            Ok(format!("Removed SSH host {name} from {}.", project_file.display()))
        }
        ["remove", ..] => Err("Usage: /ssh remove <name>".into()),
        _ => Err(USAGE.into()),
    }
}
