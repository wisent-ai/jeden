use serde_json::Value;

pub type MigrationTransform = fn(&mut Value) -> Result<(), String>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompatibilityWindow {
    pub oldest_readable: u32,
    pub newest_readable: u32,
    pub rollback_floor: u32,
}

#[derive(Clone, Copy)]
pub struct MigrationStep {
    pub name: &'static str,
    pub from: u32,
    pub to: u32,
    pub apply: MigrationTransform,
}

#[derive(Clone)]
pub struct MigrationPlan {
    pub store: &'static str,
    pub from: u32,
    pub to: u32,
    #[allow(dead_code)]
    pub reversible: bool,
    pub preflight: fn(&Value) -> Result<(), String>,
    pub steps: &'static [MigrationStep],
    pub compatibility_window: CompatibilityWindow,
}

impl MigrationPlan {
    pub fn validate(&self) -> Result<(), String> {
        if self.from > self.to {
            return Err(format!("{} migration range is inverted", self.store));
        }
        if self.compatibility_window.oldest_readable > self.from
            || self.compatibility_window.newest_readable < self.to
            || self.compatibility_window.rollback_floor > self.to
        {
            return Err(format!(
                "{} compatibility window does not cover the plan",
                self.store
            ));
        }
        let mut version = self.from;
        for step in self.steps {
            if step.from != version || step.to != version.saturating_add(1) {
                return Err(format!(
                    "{} migration step {} is not contiguous",
                    self.store, step.name
                ));
            }
            version = step.to;
        }
        if version != self.to {
            return Err(format!(
                "{} migration steps end at {}, expected {}",
                self.store, version, self.to
            ));
        }
        Ok(())
    }
}
