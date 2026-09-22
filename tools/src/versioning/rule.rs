//! The fleet's versioning rule (github.com/lbartoszcze/AutoVersion, SPEC.md at
//! v0.1.0): given the published version, the published surface and the
//! candidate surface, what kind of change this is and what the next version
//! is. The rule's SPEC keeps one small implementation per consumer language,
//! held identical by its shared fixtures; tests/versioning/ runs those
//! fixtures against this port through its command line.

use std::collections::BTreeSet;
use std::fmt;

/// A refusal, named the way the rule's fixtures name it.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) name: &'static str,
    pub(crate) message: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "refused ({}): {}", self.name, self.message)
    }
}

fn refuse(name: &'static str, message: String) -> Refusal {
    Refusal { name, message }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Breaking,
    Additive,
    Internal,
}

impl Change {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Breaking => "breaking",
            Self::Additive => "additive",
            Self::Internal => "internal",
        }
    }
}

struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A segment that survives a URL path and a filesystem key unchanged.
fn canonical(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

impl Version {
    fn parse(value: &str) -> Result<Self, Refusal> {
        if !canonical(value) {
            return Err(refuse(
                "not-canonical",
                format!("{value:?} is not a canonical coordinate: expected a non-empty segment of alphanumerics, '.', '_' and '-', with no surrounding whitespace"),
            ));
        }
        let slots = value.split('.').collect::<Vec<_>>();
        let [major, minor, patch] = slots.as_slice() else {
            return Err(refuse(
                "not-a-triple",
                format!("{value:?} is not a major.minor.patch triple, so there is no slot to advance; name the next version explicitly"),
            ));
        };
        let numeric = |slot: &str| {
            (!slot.is_empty() && slot.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| slot.parse::<u64>().ok())
                .flatten()
        };
        match (numeric(major), numeric(minor), numeric(patch)) {
            (Some(major), Some(minor), Some(patch)) => Ok(Self { major, minor, patch }),
            _ => Err(refuse(
                "not-numeric",
                format!("{value:?} has a non-numeric slot, so advancing it would invent an ordering; name the next version explicitly"),
            )),
        }
    }

    /// The version this change produces. While the major slot is zero, the
    /// minor slot carries compatibility.
    fn advance(&self, change: Change) -> Self {
        let unstable = self.major == 0;
        match (change, unstable) {
            (Change::Breaking, true) => Self {
                major: 0,
                minor: self.minor + 1,
                patch: 0,
            },
            (Change::Breaking, false) => Self {
                major: self.major + 1,
                minor: 0,
                patch: 0,
            },
            (Change::Additive, false) => Self {
                major: self.major,
                minor: self.minor + 1,
                patch: 0,
            },
            _ => Self {
                major: self.major,
                minor: self.minor,
                patch: self.patch + 1,
            },
        }
    }
}

pub(crate) struct Decision {
    pub(crate) current: String,
    pub(crate) change: Change,
    pub(crate) next: String,
    pub(crate) removed: Vec<String>,
    pub(crate) added: Vec<String>,
}

fn surface(names: &[String], side: &str) -> Result<BTreeSet<String>, Refusal> {
    let collected = names.iter().cloned().collect::<BTreeSet<_>>();
    if collected.is_empty() {
        return Err(refuse(
            "empty-surface",
            format!("the {side} surface is empty, which is far more likely to be a broken extractor than a product that promises nothing"),
        ));
    }
    Ok(collected)
}

/// The whole answer. A declared break may only escalate the class.
pub(crate) fn decide(
    current: &str,
    published: &[String],
    candidate: &[String],
    declared_breaking: bool,
) -> Result<Decision, Refusal> {
    let version = Version::parse(current)?;
    let before = surface(published, "published")?;
    let after = surface(candidate, "candidate")?;
    let removed = before.difference(&after).cloned().collect::<Vec<_>>();
    let added = after.difference(&before).cloned().collect::<Vec<_>>();
    let change = if declared_breaking || !removed.is_empty() {
        Change::Breaking
    } else if !added.is_empty() {
        Change::Additive
    } else {
        Change::Internal
    };
    Ok(Decision {
        current: version.to_string(),
        change,
        next: version.advance(change).to_string(),
        removed,
        added,
    })
}
