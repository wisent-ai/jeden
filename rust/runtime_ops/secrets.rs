use super::security::{ExecutionGrant, GrantError};
use std::collections::BTreeMap;
#[derive(Default)]
pub struct SecretBroker {
    values: BTreeMap<String, Vec<u8>>,
}
impl SecretBroker {
    pub fn insert(&mut self, name: impl Into<String>, value: Vec<u8>) {
        self.values.insert(name.into(), value);
    }
    pub fn expose<T>(
        &self,
        grant: &ExecutionGrant,
        name: &str,
        use_secret: impl FnOnce(&[u8]) -> T,
    ) -> Result<T, GrantError> {
        if grant.is_expired() {
            return Err(GrantError::Expired);
        }
        if !grant.secrets.names.contains(name) {
            return Err(GrantError::SecretDenied(name.into()));
        }
        let value = self
            .values
            .get(name)
            .ok_or_else(|| GrantError::SecretDenied(format!("{name} is unavailable")))?;
        Ok(use_secret(value))
    }
}
