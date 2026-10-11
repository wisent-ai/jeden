//! A conversation's model calls belong to its session, so a route Brama keeps
//! a pool of accounts for runs on the session's lease
//! (`crate::model_router::lease`), and the conversation's end gives the
//! lease back.

use super::*;

impl Conversation {
    /// The session these calls belong to: its directory's name, as the
    /// session store and the SessionStart hook name it.
    pub(crate) fn session_id(&self) -> Result<String, String> {
        let session = self.recorder.path();
        session
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "the session directory {} has no name to lease a Brama account for",
                    session.display()
                )
            })
    }

    /// The router for this conversation's calls, naming its session.
    pub(super) fn session_router(
        &self,
        config: &Config,
        args: &Args,
    ) -> Result<ChatConfig, String> {
        let mut router = model_router_config(config, args);
        router.session_id = Some(self.session_id()?);
        Ok(router)
    }
}

impl Drop for Conversation {
    /// The session ended in this process: the accounts it leased from Brama
    /// carry one session fewer. A release Brama refuses is said, with its
    /// cause; `jeden lease release --session <id>` ends it later.
    fn drop(&mut self) {
        let released = self
            .session_id()
            .and_then(|session| crate::model_router::lease::release_held(&session));
        if let Err(error) = released {
            eprintln!("jeden: {error}");
        }
    }
}
