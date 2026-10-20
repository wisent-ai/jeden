//! A declared vocabulary is a unit enum whose words are its serde names:
//! the one place a word is read into its variant and a variant is spelled
//! back, so no caller keeps its own list of the words.

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

/// The value of the vocabulary `T` that `word` spells, or the deserializer's
/// refusal, which names the word and every spelling `T` accepts.
pub(crate) fn named<T: DeserializeOwned>(word: &str) -> Result<T, String> {
    serde_json::from_value(Value::String(word.trim().to_owned()))
        .map_err(|refusal| refusal.to_string())
}

/// How a vocabulary value is spelled: its serde name.
pub(crate) fn spelling<T: Serialize>(value: T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        other => unreachable!("a unit variant serializes as its name, not {other:?}"),
    }
}
