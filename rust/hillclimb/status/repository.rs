//! A recorded proposal identity, not a filesystem target or an execution scope.
#[derive(Clone, Copy)]
pub(crate) enum RepositorySelection<'a> {
    All,
    Missing,
    Named(&'a str),
}

impl<'a> RepositorySelection<'a> {
    pub(crate) fn new(name: Option<&'a str>, missing: bool) -> Result<Self, String> {
        match (name, missing) {
            (Some(name), true) => Err(format!(
                "recorded repository {name:?} conflicts with selecting records without a repository identity; choose one selection"
            )),
            (Some(name), false) => Ok(Self::Named(name)),
            (None, true) => Ok(Self::Missing),
            (None, false) => Ok(Self::All),
        }
    }

    pub(super) fn matches(self, recorded: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Missing => recorded.is_none(),
            Self::Named(name) => recorded == Some(name),
        }
    }

    pub(super) fn name(self) -> Option<&'a str> {
        match self {
            Self::Named(name) => Some(name),
            Self::All | Self::Missing => None,
        }
    }

    pub(super) fn is_missing(self) -> bool {
        matches!(self, Self::Missing)
    }
}
