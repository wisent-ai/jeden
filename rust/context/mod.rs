pub(crate) mod discovery;
mod rules;
mod secrets;

use std::path::Path;

use serde_json::Value;

use crate::cli::config::Config;

use discovery::ContextBundle;
pub(crate) use rules::RuleRegistry;
pub(crate) use secrets::SecretPolicy;

#[derive(Debug, Clone)]
pub(crate) struct ContextPolicy {
    bundle: ContextBundle,
    rules: RuleRegistry,
    secrets: SecretPolicy,
}

impl ContextPolicy {
    pub(crate) fn load(cwd: &Path, config: &Config) -> Result<Self, String> {
        let bundle = discovery::discover_context(cwd, &config.context, &config.rules)?;
        let rules = RuleRegistry::from_bundle(&bundle);
        let secrets = SecretPolicy::load(cwd, &config.secrets)?;
        Ok(Self {
            bundle,
            rules,
            secrets,
        })
    }

    pub(crate) fn system_injection(&self) -> String {
        format!(
            "{}{}",
            self.bundle.render_for_prompt(),
            self.rules.render_for_prompt()
        )
    }

    pub(crate) fn protect_model_text(&self, text: &str) -> String {
        self.secrets.protect_text(text)
    }

    pub(crate) fn protect_model_messages(&self, messages: &[Value]) -> Vec<Value> {
        self.secrets.protect_messages(messages)
    }
}

/// Single narrow integration API for provider-bound messages. Callers retain
/// their original local history and provenance; only the returned copy is safe
/// to pass to a model provider.
pub(crate) fn prepare_model_messages(
    cwd: &Path,
    config: &Config,
    messages: &[Value],
) -> Result<Vec<Value>, String> {
    Ok(ContextPolicy::load(cwd, config)?.protect_model_messages(messages))
}
