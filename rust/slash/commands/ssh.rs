use crate::slash::SlashContext;
use crate::slash::common::{
    merged_config, project_config_path, read_json_value, split_args, write_json_value,
};
use serde_json::{Map, Value};

fn ssh_hosts_from(config: &Value) -> Option<&Map<String, Value>> {
    config.get("sshHosts").and_then(Value::as_object)
        .or_else(|| config.get("ssh").and_then(|ssh| ssh.get("hosts")).and_then(Value::as_object))
        .or_else(|| config.get("ssh").and_then(Value::as_object))
}

fn ssh_host_value(target: &str, options: &[String]) -> Option<Value> {
    if options.is_empty() { return Some(Value::String(target.to_string())); }
    let mut host = Map::new();
    host.insert("host".into(), Value::String(target.to_string()));
    for option in options {
        let (key, value) = option.split_once('=')?;
        if key.is_empty() { return None; }
        host.insert(key.to_string(), Value::String(value.to_string()));
    }
    Some(Value::Object(host))
}

pub(crate) fn handle_ssh(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let empty: &[String] = &[];
    let (verb, after_verb) = argv
        .split_first()
        .map(|(word, rest)| (word.as_str(), rest))
        .unwrap_or(("list", empty));
    let (name, after_name) = after_verb
        .split_first()
        .map(|(word, rest)| (word.as_str(), rest))
        .unwrap_or(("", empty));
    let (target, options) = after_name
        .split_first()
        .map(|(word, rest)| (word.as_str(), rest))
        .unwrap_or(("", empty));
    let project_file = project_config_path(context.cwd);
    if verb == "list" {
        let config = merged_config(context.cwd);
        let Some(hosts) = ssh_hosts_from(&config) else {
            return Ok("No SSH hosts configured in ~/.jeden/config.json or <cwd>/.jeden/config.json (sshHosts).".into());
        };
        let mut names = hosts.keys().cloned().collect::<Vec<_>>();
        names.sort();
        if names.is_empty() {
            return Ok("No SSH hosts configured in ~/.jeden/config.json or <cwd>/.jeden/config.json (sshHosts).".into());
        }
        let mut lines = Vec::new();
        for host in names {
            let value = hosts.get(&host).cloned().unwrap_or(Value::Null);
            let rendered = match value.as_str() {
                Some(text) => text.to_string(),
                None => serde_json::to_string(&value).map_err(|error| error.to_string())?,
            };
            lines.push(format!("{}\t{}", host, rendered));
        }
        return Ok(lines.join("\n"));
    }
    if verb == "help" {
        return Ok("Usage: /ssh list | add <name> <target> [key=value ...] | remove <name>. Hosts are stored in <cwd>/.jeden/config.json under sshHosts.".into());
    }
    if verb == "add" {
        if name.is_empty() || target.is_empty() { return Err("Usage: /ssh add <name> <target> [key=value ...]".into()); }
        let value = ssh_host_value(target, options).ok_or_else(|| "Usage: /ssh add <name> <target> [key=value ...]".to_string())?;
        let mut project = read_json_value(&project_file);
        if !project.is_object() { project = Value::Object(Map::new()); }
        let object = project.as_object_mut().expect("project config object");
        let hosts = object.entry("sshHosts").or_insert_with(|| Value::Object(Map::new()));
        if !hosts.is_object() { *hosts = Value::Object(Map::new()); }
        hosts.as_object_mut().expect("sshHosts object").insert(name.to_string(), value);
        write_json_value(&project_file, &project)?;
        return Ok(format!("Added SSH host {} to {}.", name, project_file.display()));
    }
    if verb == "remove" {
        if name.is_empty() { return Err("Usage: /ssh remove <name>".into()); }
        let effective = merged_config(context.cwd);
        let mut project = read_json_value(&project_file);
        let in_project = project.get("sshHosts").and_then(Value::as_object).and_then(|hosts| hosts.get(name)).is_some();
        if !in_project {
            if ssh_hosts_from(&effective).and_then(|hosts| hosts.get(name)).is_some() {
                return Err(format!("SSH host {} is not in <cwd>/.jeden/config.json. Remove it from ~/.jeden/config.json or the config file that defines it.", name));
            }
            return Err(format!("SSH host not found: {}", name));
        }
        if let Some(hosts) = project.get_mut("sshHosts").and_then(Value::as_object_mut) { hosts.remove(name); }
        write_json_value(&project_file, &project)?;
        return Ok(format!("Removed SSH host {} from {}.", name, project_file.display()));
    }
    Err("Usage: /ssh list | add <name> <target> [key=value ...] | remove <name> | help".into())
}
