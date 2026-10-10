//! One response contract for CLI, RPC and the model tool, without copying
//! source diagnostics into a second in-memory collection.
use serde::{ser::SerializeMap, Serialize, Serializer};

use super::super::Advice;

struct Unanswered<'a>(&'a Advice);

impl Serialize for Unanswered<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.unavailable_sources())
    }
}

impl Serialize for Advice {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut response = serializer.serialize_map(None)?;
        response.serialize_entry("query", &self.query)?;
        response.serialize_entry("limit", &self.limit)?;
        response.serialize_entry("recommendations", &self.recommendations)?;
        response.serialize_entry("sources", &self.sources)?;
        response.serialize_entry("unanswered", &Unanswered(self))?;
        response.end()
    }
}
