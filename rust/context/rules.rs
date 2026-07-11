use super::discovery::{ContextBundle, ContextEntry, ContextKind};

#[derive(Debug, Clone, Default)]
pub(crate) struct RuleRegistry {
    rules: Vec<ContextEntry>,
}

impl RuleRegistry {
    pub(crate) fn from_bundle(bundle: &ContextBundle) -> Self {
        let mut rules = bundle
            .entries
            .iter()
            .filter(|entry| entry.kind == ContextKind::Rule && entry.always_apply)
            .cloned()
            .collect::<Vec<_>>();
        rules.sort_by_key(|rule| rule.precedence);
        Self { rules }
    }

    pub(crate) fn always_apply(&self) -> &[ContextEntry] {
        &self.rules
    }

    pub(crate) fn render_for_prompt(&self) -> String {
        if self.rules.is_empty() {
            return String::new();
        }
        let rules = self
            .rules
            .iter()
            .map(|rule| {
                let provenance = rule
                    .provenance
                    .iter()
                    .map(|source| source.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" -> ");
                format!(
                    "[sticky-rule:{}; precedence={}; provenance={}]\n{}",
                    rule.id, rule.precedence, provenance, rule.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        format!("\n\nAlways-applied sticky rules:\n{rules}")
    }
}
