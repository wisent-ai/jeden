//! The text `jeden asks` prints for the values its `--json` output carries.
//! Each field the value is built with is read by name; a field the value
//! does not carry is refused naming it, never printed as empty.

use serde_json::Value;

/// The text field `key` of `value`, refused naming the field and the value.
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("the asks value carries no text {key}: {value}"))
}

/// The array field `key` of `value`, refused naming the field and the value.
fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value[key]
        .as_array()
        .ok_or_else(|| format!("the asks value carries no list {key}: {value}"))
}

pub(super) fn register(value: &Value) -> Result<String, String> {
    let asks = list(value, "asks")?;
    if asks.is_empty() {
        return Ok(format!(
            "Nothing has been asked of you. Register: {}",
            text(value, "register")?
        ));
    }
    let lines = asks.iter().map(summary).collect::<Result<Vec<_>, _>>()?;
    Ok(lines.join("\n"))
}

fn summary(ask: &Value) -> Result<String, String> {
    let asked = ask["timesAsked"]
        .as_u64()
        .ok_or_else(|| format!("the ask carries no timesAsked count: {ask}"))?;
    let asked = usize::try_from(asked).map_err(|error| {
        format!("the ask's timesAsked {asked} is no count of this machine: {error}")
    })?;
    let mut line = format!(
        "{} [{}] asked {} since {}: {}",
        text(ask, "id")?,
        text(ask, "status")?,
        super::super::times(asked),
        text(ask, "askedAt")?,
        text(ask, "ask")?,
    );
    if let Some(follows) = ask["follows"].as_str() {
        line.push_str(&format!(
            "\n  Asked again after ask {follows} because an operation failed after its answer"
        ));
    }
    match ask["answer"].as_object() {
        Some(_) => line.push_str(&format!(
            "\n  Your answer ({}): {}",
            text(&ask["answer"], "answeredAt")?,
            text(&ask["answer"], "text")?
        )),
        None => line.push_str(&format!(
            "\n  Answer with: jeden asks answer {} --text <answer>",
            text(ask, "id")?
        )),
    }
    if ask["oko"].is_object() {
        line.push_str(&format!("\n  Oko: {}", text(&ask["oko"], "detail")?));
    }
    Ok(line)
}

pub(super) fn show(value: &Value) -> Result<String, String> {
    let mut lines = vec![summary(value)?];
    for place in list(value, "occurrences")? {
        let mut line = format!(
            "  Asked {} by {} in {}",
            text(place, "askedAt")?,
            text(place, "source")?,
            text(place, "sessionPath")?,
        );
        if let Some(task) = place["taskId"].as_str() {
            line.push_str(&format!(" for task {task}"));
        }
        if let Some(status) = place["taskStatus"].as_str() {
            line.push_str(&format!(" (task {status})"));
        }
        if let Some(state) = place["taskState"].as_str() {
            line.push_str(&format!(" ({state})"));
        }
        if place["answeredFromRegister"] == Value::Bool(true) {
            line.push_str(" — answered from the register, not asked again");
        }
        lines.push(line);
        lines.push(format!("    {}", text(place, "wording")?));
    }
    Ok(lines.join("\n"))
}

pub(super) fn answer(value: &Value) -> Result<String, String> {
    let mut lines = vec![format!(
        "Recorded your answer to ask {}: {}",
        text(&value["ask"], "id")?,
        text(&value["ask"]["answer"], "text")?
    )];
    let deliveries = list(value, "deliveries")?;
    if deliveries.is_empty() {
        lines.push("No task waits on this ask now.".into());
    }
    for delivery in deliveries {
        lines.push(format!(
            "  {} in {}: {} — {}",
            text(delivery, "taskId")?,
            text(delivery, "sessionPath")?,
            text(delivery, "outcome")?,
            text(delivery, "detail")?,
        ));
    }
    Ok(lines.join("\n"))
}
