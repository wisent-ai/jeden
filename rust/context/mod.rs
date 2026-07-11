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

    pub(crate) fn rules(&self) -> &RuleRegistry {
        &self.rules
    }

    pub(crate) fn bundle(&self) -> &ContextBundle {
        &self.bundle
    }

    pub(crate) fn secret_provenance(&self) -> Vec<&str> {
        self.secrets.provenance()
    }

    pub(crate) fn protect_model_text(&self, text: &str) -> String {
        self.secrets.protect_text(text)
    }

    pub(crate) fn protect_model_value(&self, value: &Value) -> Value {
        self.secrets.protect_json(value)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::config::{
        AlwaysApplyRuleConfig, Config, ContextConfig, RulesConfig, SecretMode, SecretsConfig,
    };
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "jeden-context-policy-{name}-{}-{}",
                std::process::id(),
                NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create context-policy test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(root: &Path) {
        fs::create_dir(root.join(".git")).expect("create project marker");
    }

    fn config(max_bytes: usize, max_tokens: usize) -> Config {
        Config {
            context: ContextConfig {
                max_bytes,
                max_tokens,
            },
            secrets: SecretsConfig {
                mode: SecretMode::Redact,
                replacement: "<protected>".to_string(),
                min_length: 1,
                values: Vec::new(),
                environment: Vec::new(),
                files: Vec::new(),
                discover_environment: false,
            },
            ..Config::default()
        }
    }

    #[test]
    fn context_policy_precedence_is_root_then_descendant_then_configured_sticky() {
        let fixture = TempDir::new("precedence");
        let root = fixture.path().join("project");
        let descendant = root.join("crates/leaf");
        fs::create_dir_all(descendant.join(".jeden")).expect("create descendant");
        project(&root);
        fs::write(root.join("JEDEN.md"), "root rule").expect("write root rule");
        fs::write(descendant.join("CLAUDE.md"), "descendant rule")
            .expect("write descendant rule");
        fs::write(descendant.join(".jeden/context.md"), "descendant context")
            .expect("write descendant context");

        let mut config = config(4_096, 1_024);
        config.rules = RulesConfig {
            always_apply: vec![AlwaysApplyRuleConfig {
                id: "configured-sticky".to_string(),
                content: Some("configured rule".to_string()),
                source: None,
            }],
        };

        let policy = ContextPolicy::load(&descendant, &config).expect("load context policy");
        let ordered = policy
            .bundle()
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry.precedence, entry.always_apply))
            .collect::<Vec<_>>();

        assert_eq!(
            ordered,
            vec![
                ("JEDEN.md", 0, true),
                ("CLAUDE.md", 1, true),
                (".jeden/context.md", 2, false),
                ("configured-sticky", 3, true),
            ]
        );
    }

    #[test]
    fn context_policy_import_cycle_is_a_hard_error_with_the_cycle_chain() {
        let fixture = TempDir::new("import-cycle");
        let root = fixture.path().join("project");
        fs::create_dir_all(&root).expect("create project");
        project(&root);
        let rules = root.join("RULES.md");
        let nested = root.join("nested.md");
        fs::write(&rules, "@import nested.md\n").expect("write root import");
        fs::write(&nested, "@import RULES.md\n").expect("write cyclic import");

        let error = ContextPolicy::load(&root, &config(4_096, 1_024))
            .expect_err("cyclic imports must be rejected");

        assert_eq!(
            error,
            format!(
                "context import cycle: {} -> {} -> {}",
                rules.canonicalize().unwrap().display(),
                nested.canonicalize().unwrap().display(),
                rules.canonicalize().unwrap().display()
            )
        );
    }

    #[test]
    fn context_policy_import_depth_accepts_256_files_and_rejects_the_257th() {
        let fixture = TempDir::new("import-depth");
        let root = fixture.path().join("project");
        let chain = root.join("chain");
        fs::create_dir_all(&chain).expect("create import chain");
        project(&root);
        fs::write(root.join("RULES.md"), "@import chain/000.md\n")
            .expect("write chain root");
        for index in 0..255 {
            let content = if index == 254 {
                "terminal\n".to_string()
            } else {
                format!("@import {:03}.md\n", index + 1)
            };
            fs::write(chain.join(format!("{index:03}.md")), content)
                .expect("write import chain member");
        }

        ContextPolicy::load(&root, &config(16_384, 4_096))
            .expect("256 context files remain within the hard limit");

        fs::write(chain.join("254.md"), "@import 255.md\n").expect("extend import chain");
        let rejected = chain.join("255.md");
        fs::write(&rejected, "beyond limit\n").expect("write over-limit import");
        let error = ContextPolicy::load(&root, &config(16_384, 4_096))
            .expect_err("257th context file must be rejected");

        assert_eq!(
            error,
            format!(
                "context import limit exceeded at {} (maximum 256 files)",
                rejected.canonicalize().unwrap().display()
            )
        );
    }

    #[test]
    fn context_policy_context_import_cannot_escape_the_project_path_jail() {
        let fixture = TempDir::new("context-jail");
        let root = fixture.path().join("project");
        fs::create_dir_all(&root).expect("create project");
        project(&root);
        let outside = fixture.path().join("outside.md");
        fs::write(&outside, "outside policy").expect("write outside context");
        fs::write(root.join("RULES.md"), "@import ../outside.md\n")
            .expect("write escaping import");

        let error = ContextPolicy::load(&root, &config(4_096, 1_024))
            .expect_err("escaping context import must be rejected");

        assert_eq!(
            error,
            format!(
                "context import {} escapes path jail {}",
                outside.canonicalize().unwrap().display(),
                root.canonicalize().unwrap().display()
            )
        );
    }

    #[test]
    fn context_policy_secret_file_cannot_escape_the_project_path_jail() {
        let fixture = TempDir::new("secret-jail");
        let root = fixture.path().join("project");
        fs::create_dir_all(&root).expect("create project");
        project(&root);
        let outside = fixture.path().join("outside.env");
        fs::write(&outside, "TOKEN=outside-secret").expect("write outside secret");
        let mut config = config(4_096, 1_024);
        config.secrets.files.push(PathBuf::from("../outside.env"));

        let error = ContextPolicy::load(&root, &config)
            .expect_err("escaping secret file must be rejected");

        assert_eq!(
            error,
            format!(
                "configured secret file {} escapes path jail {}",
                outside.canonicalize().unwrap().display(),
                root.canonicalize().unwrap().display()
            )
        );
    }

    #[test]
    fn context_policy_budget_warning_and_import_provenance_name_the_truncated_source() {
        let fixture = TempDir::new("budget-provenance");
        let root = fixture.path().join("project");
        fs::create_dir_all(&root).expect("create project");
        project(&root);
        let rules = root.join("RULES.md");
        let imported = root.join("imported.md");
        fs::write(&rules, "@import imported.md\n").expect("write root import");
        fs::write(&imported, "0123456789").expect("write imported context");

        let policy = ContextPolicy::load(&root, &config(8, 100)).expect("load budgeted policy");
        let entry = &policy.bundle().entries[0];
        let canonical_rules = rules.canonicalize().unwrap();
        let canonical_imported = imported.canonicalize().unwrap();

        assert_eq!(entry.content, "01234567\n");
        assert_eq!(entry.provenance.len(), 2);
        assert_eq!(entry.provenance[0].path, canonical_rules);
        assert_eq!(entry.provenance[0].imported_by, None);
        assert_eq!(entry.provenance[1].path, canonical_imported);
        assert_eq!(
            entry.provenance[1].imported_by.as_deref(),
            Some(canonical_rules.as_path())
        );
        assert_eq!(
            policy.bundle().warnings,
            vec![format!(
                "budget exceeded while reading {}: included 8 of 10 bytes (limits: 8 bytes, ~100 tokens)",
                canonical_imported.display()
            )]
        );
    }

    #[test]
    fn context_policy_redacts_system_user_tool_and_nested_json_secrets() {
        let fixture = TempDir::new("redaction");
        let root = fixture.path().join("project");
        fs::create_dir_all(&root).expect("create project");
        project(&root);
        let mut config = config(4_096, 1_024);
        config.secrets.values.push("s3cr3t-token".to_string());
        let policy = ContextPolicy::load(&root, &config).expect("load secret policy");
        let messages = vec![
            json!({"role": "system", "content": "system s3cr3t-token"}),
            json!({"role": "user", "content": "user s3cr3t-token"}),
            json!({
                "role": "tool",
                "content": {
                    "nested": ["safe", {"s3cr3t-token-key": "value s3cr3t-token"}]
                }
            }),
        ];

        let protected = policy.protect_model_messages(&messages);

        assert_eq!(
            protected,
            vec![
                json!({"role": "system", "content": "system <protected>"}),
                json!({"role": "user", "content": "user <protected>"}),
                json!({
                    "role": "tool",
                    "content": {
                        "nested": ["safe", {"<protected>-key": "value <protected>"}]
                    }
                }),
            ]
        );
    }

    #[test]
    fn context_policy_outbound_preparation_returns_a_protected_clone_without_mutating_history() {
        let fixture = TempDir::new("outbound-clone");
        let root = fixture.path().join("project");
        fs::create_dir_all(&root).expect("create project");
        project(&root);
        let mut config = config(4_096, 1_024);
        config.secrets.values.push("local-only-secret".to_string());
        let messages = vec![json!({
            "role": "user",
            "content": "retain local-only-secret in local history"
        })];
        let original = messages.clone();

        let outbound = prepare_model_messages(&root, &config, &messages)
            .expect("prepare provider-bound message clone");

        assert_eq!(messages, original);
        assert_eq!(
            outbound,
            vec![json!({
                "role": "user",
                "content": "retain <protected> in local history"
            })]
        );
    }
}
