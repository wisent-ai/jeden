//! Turning advice into the three shapes a reader needs: the terminal block a
//! person reads, the prompt block a model receives, and the probe object a
//! GUI or script consumes.

use serde_json::{Map, Value};

use super::{Advice, SourceStatus};

pub(crate) fn availability_word(available: bool) -> &'static str {
    if available {
        "available"
    } else {
        "unavailable"
    }
}

pub(crate) fn render_text(advice: &Advice) -> String {
    let mut out = String::new();
    if advice.recommendations.is_empty() {
        out.push_str("No context recommendation matched this query.\n");
    }
    for (index, hit) in advice.recommendations.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}] {} — {} (score {:.1})\n",
            index + 1,
            hit.source,
            hit.locator,
            hit.title,
            hit.score
        ));
        for line in hit.snippet.lines() {
            out.push_str("     ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("\nSources:\n");
    for status in &advice.sources {
        out.push_str(&format!(
            "- {}: {} — {} (considered {}, returned {}, {} ms)\n",
            status.source,
            availability_word(status.available),
            status.detail,
            status.considered,
            status.returned,
            status.elapsed_ms
        ));
    }
    out
}

/// The block appended to a turn's prompt. `None` when there is nothing to
/// say, so an empty advisory never costs a token.
pub(crate) fn prompt_section(advice: &Advice, max_chars: usize) -> Option<String> {
    if advice.recommendations.is_empty() && advice.unavailable_sources().is_empty() {
        return None;
    }
    let mut out = String::from(
        "[Context recommendations]\nRead these before searching; each locator is exact. They are ranked matches, not verified answers.\n",
    );
    for hit in &advice.recommendations {
        let line = format!(
            "- [{}] {} — {}\n  {}\n",
            hit.source,
            hit.locator,
            hit.title,
            hit.snippet.replace('\n', " ")
        );
        if out.chars().count() + line.chars().count() > max_chars {
            break;
        }
        out.push_str(&line);
    }
    let missing = advice.unavailable_sources();
    if !missing.is_empty() {
        let mut note = String::from("Sources that answered nothing:\n");
        for status in missing {
            note.push_str(&format!("- {}: {}\n", status.source, status.detail));
        }
        if out.chars().count() + note.chars().count() <= max_chars {
            out.push_str(&note);
        }
    }
    if advice.recommendations.is_empty() {
        out.push_str(
            "No documentation, memory, transcript or ground-truth match was found for this task.\n",
        );
    }
    Some(out)
}

/// A source's status plus the fields only that source has. One shape, so a
/// caller reads `available` and `detail` the same way for every source.
pub(crate) fn probe_value(status: &SourceStatus, extras: Vec<(&str, Value)>) -> Value {
    let mut object = match serde_json::to_value(status) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for (key, value) in extras {
        object.insert(key.to_string(), value);
    }
    Value::Object(object)
}
