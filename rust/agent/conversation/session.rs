//! A conversation's model calls belong to its session: a route Brama keeps a
//! pool of accounts for runs on the session's lease
//! (`crate::model_router::lease`), the conversation's end gives the lease
//! back, and the session's chat memory (`crate::chat_memory`) is what a step
//! sends when the configuration turns it on.

use super::*;
use crate::chat_memory::session::ChatMemory;
use crate::chat_memory::{Settings, Summarize};
use std::sync::Arc;

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

    /// The messages a step sends: the chat memory's view of this
    /// conversation, opening the memory at the first step when the
    /// configuration turns it on, or the conversation's own messages when it
    /// is off. Only the agent's own conversation writes to its chat — the one
    /// that keeps the task's completion, or a model-only run; an intake or
    /// acceptance inspection and a Pursuit stage are the harness's machinery,
    /// and their messages stay out of it.
    pub(super) fn step_messages(
        &mut self,
        config: &Config,
        args: &Args,
    ) -> Result<Vec<Value>, String> {
        let own = !self.inspection && (self.manages_completion || args.model_only);
        if self.memory.is_none() && own {
            if let Some(settings) = crate::chat_memory::settings(&args.cwd)? {
                let summarize = self.summarizer(&settings, config, args)?;
                self.memory = Some(ChatMemory::open(
                    &self.recorder.path(),
                    &args.cwd,
                    &settings,
                    summarize,
                    &self.messages,
                )?);
            }
        }
        match &mut self.memory {
            Some(memory) => {
                memory.sync(&self.messages)?;
                memory.outbound(&self.messages)
            }
            None => Ok(self.messages.clone()),
        }
    }

    /// After a step, the chat memory writes ahead the summaries its next
    /// fold will need, on its own thread.
    pub(super) fn build_memory_ahead(&mut self) -> Result<(), String> {
        if let Some(memory) = &mut self.memory {
            memory.sync(&self.messages)?;
            memory.build_in_background();
        }
        Ok(())
    }

    /// Before the conversation records into a new session directory (a
    /// `/reset`, a resume, `/fork` or `/branch`), what it said so far joins
    /// its chat and the chat is let go; the new session opens the same chat
    /// at its next step and goes on from there.
    pub(super) fn leave_memory(&mut self) -> Result<(), String> {
        if let Some(memory) = &mut self.memory {
            memory.sync(&self.messages)?;
        }
        self.memory = None;
        Ok(())
    }

    /// The cheap model named by `chatMemory.summaryModel` writes a summary
    /// within the given byte ruler on the selected route. A model the
    /// selected provider's catalog refuses is rejected before summarizing.
    fn summarizer(
        &self,
        settings: &Settings,
        config: &Config,
        args: &Args,
    ) -> Result<Arc<Summarize>, String> {
        let summary_args = Args {
            model: Some(settings.summary_model.clone()),
            ..args.clone()
        };
        let mut router = model_router_config(config, &summary_args);
        router.session_id = Some(self.session_id()?);
        if let Some(error) = &router.config_error {
            return Err(format!(
                "chatMemory.summaryModel `{}` cannot write the chat memory's summaries: {error}",
                settings.summary_model
            ));
        }
        // The chat memory writes the compaction's whole conversation (its
        // instructions, the task with its ruler, and any "too long" retry);
        // the cheap model answers the line.
        Ok(Arc::new(move |messages: &[Value]| {
            chat_completion(&router, messages.to_vec(), None, &[])
                .map(|completion| completion.content)
        }))
    }
}

impl Drop for Conversation {
    /// A session that held Brama leases releases them on exit, reporting any
    /// release refusal. A direct-provider session holds none.
    fn drop(&mut self) {
        // The answer a turn ended with joined the conversation after its
        // last step; it joins the chat log now, with any failure said.
        if let Some(memory) = &mut self.memory {
            if let Err(error) = memory.sync(&self.messages) {
                eprintln!(
                    "jeden: the chat memory did not keep the conversation's last messages: {error}"
                );
            }
        }
        let released = self
            .session_id()
            .and_then(|session| crate::model_router::lease::release_held(&session));
        if let Err(error) = released {
            eprintln!("jeden: {error}");
        }
    }
}
