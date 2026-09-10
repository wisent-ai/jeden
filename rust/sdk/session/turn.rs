use super::*;

impl AgentSession {
    pub(super) fn dispatch_prompt(
        &self,
        request: PromptRequest,
        continuing: bool,
    ) -> Result<PromptResult, String> {
        if request.request_id.trim().is_empty() {
            return Err("request_id must not be empty".into());
        }
        if request.prompt.trim().is_empty() {
            return Err("prompt must not be empty".into());
        }
        if self.inner.disposed.load(Ordering::Acquire) {
            return Err("session disposed".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut active = self
                .inner
                .active
                .lock()
                .map_err(|_| "active request lock poisoned")?;
            if active.contains_key(&request.request_id) {
                return Err(format!("request already active: {}", request.request_id));
            }
            active.insert(request.request_id.clone(), cancel.clone());
        }
        let result = self.run_prompt(&request, cancel, continuing);
        if let Ok(mut active) = self.inner.active.lock() {
            active.remove(&request.request_id);
        }
        if let Err(message) = &result {
            let _ = self.inner.emit(SessionEvent {
                request_id: request.request_id,
                event: SessionEventKind::Error {
                    message: message.clone(),
                },
            });
        }
        result
    }
    fn run_prompt(
        &self,
        request: &PromptRequest,
        cancel: Arc<AtomicBool>,
        continuing: bool,
    ) -> Result<PromptResult, String> {
        let mut conversation_guard = self
            .inner
            .conversation
            .lock()
            .map_err(|_| "conversation lock poisoned".to_string())?;
        let conversation = conversation_guard.as_mut().ok_or("session disposed")?;
        let args = args_from_options(
            &self.inner.options,
            request.prompt.clone(),
            request.goal.clone(),
        );
        let event_error = Arc::new(Mutex::new(None::<String>));
        let request_id = request.request_id.clone();
        // Read per prompt so a mode saved from the CLI or Jeden Desktop
        // applies to the next turn of an open session.
        let policy = DisplayPolicy::for_cwd(&args.cwd);
        // A `Fn` sink with per-turn state: the turn runs on one thread, and
        // the tail is drained here once the turn is over.
        let code_filter = Rc::new(RefCell::new((!policy.code).then(CodeFilter::default)));
        let stream_filter = Rc::clone(&code_filter);

        let progress_inner = self.inner.clone();
        let progress_id = request_id.clone();
        let progress_error = event_error.clone();
        let stream_inner = self.inner.clone();
        let stream_id = request_id.clone();
        let stream_error = event_error.clone();
        let trace_inner = self.inner.clone();
        let trace_id = request_id.clone();
        let ask_inner = self.inner.clone();
        let ask_id = request_id.clone();
        let approve_inner = self.inner.clone();
        let approve_id = request_id.clone();
        let approve_error = event_error.clone();
        let goal_inner = self.inner.clone();
        let goal_id = request_id.clone();

        let mut hooks = agent::RunHooks {
            cancel,
            interactive: false,
            progress: Box::new(move |message| {
                if let Err(error) = progress_inner.emit(SessionEvent {
                    request_id: progress_id.clone(),
                    event: SessionEventKind::Status {
                        message: policy.note(message).to_string(),
                    },
                }) {
                    if let Ok(mut slot) = progress_error.lock() {
                        *slot = Some(error);
                    }
                }
            }),
            stream: Box::new(move |text| {
                let text = match stream_filter.borrow_mut().as_mut() {
                    Some(filter) => filter.push(text),
                    None => text.to_string(),
                };
                if text.is_empty() {
                    return;
                }
                if let Err(error) = stream_inner.emit(SessionEvent {
                    request_id: stream_id.clone(),
                    event: SessionEventKind::TextDelta { text },
                }) {
                    if let Ok(mut slot) = stream_error.lock() {
                        *slot = Some(error);
                    }
                }
            }),
            trace: Box::new(move |event| {
                let event = match *event {
                    agent::TraceEvent::CompletionState { state } => {
                        if let Some(path) =
                            state.get("sessionPath").and_then(serde_json::Value::as_str)
                        {
                            if let Ok(mut current) = trace_inner.session_path.write() {
                                *current = PathBuf::from(path);
                            }
                        }
                        SessionEventKind::Completion {
                            state: state.clone(),
                        }
                    }
                    agent::TraceEvent::Message { text } => SessionEventKind::AssistantMessage {
                        text: text.to_string(),
                    },
                    agent::TraceEvent::ToolCall { tool, input } if policy.tool_call_detail() => {
                        SessionEventKind::ToolCall {
                            tool: tool.to_string(),
                            input: input.clone(),
                        }
                    }
                    agent::TraceEvent::ToolResult { tool, result } if policy.tool_results => {
                        SessionEventKind::ToolResult {
                            tool: tool.to_string(),
                            result: result.clone(),
                        }
                    }
                    agent::TraceEvent::Reasoning { text } if policy.reasoning => {
                        SessionEventKind::ReasoningDelta {
                            text: text.to_string(),
                        }
                    }
                    _ => return,
                };
                // Best-effort like goal events: a hidden-by-default trace never
                // fails the prompt it decorates.
                let _ = trace_inner.emit(SessionEvent {
                    request_id: trace_id.clone(),
                    event,
                });
            }),
            ask_user: Some(Box::new(move |question, options| {
                let token = ask_inner.interaction_token("elicit");
                ask_inner.emit(SessionEvent {
                    request_id: ask_id.clone(),
                    event: SessionEventKind::Elicitation {
                        token: token.clone(),
                        question: question.to_string(),
                        options: options.to_vec(),
                    },
                })?;
                let handler = ask_inner
                    .interactions
                    .read()
                    .map_err(|_| "interaction handler lock poisoned")?
                    .clone();
                handler
                    .ok_or("elicitation requires an interaction handler")?
                    .elicit(ElicitationRequest {
                        token,
                        request_id: ask_id.clone(),
                        question: question.to_string(),
                        options: options.to_vec(),
                    })
            })),
            approve: Box::new(move |tool, detail| {
                let token = approve_inner.interaction_token("approval");
                if let Err(error) = approve_inner.emit(SessionEvent {
                    request_id: approve_id.clone(),
                    event: SessionEventKind::Approval {
                        token: token.clone(),
                        tool: tool.to_string(),
                        detail: detail.to_string(),
                    },
                }) {
                    if let Ok(mut slot) = approve_error.lock() {
                        *slot = Some(error);
                    }
                    return false;
                }
                let result = approve_inner
                    .interactions
                    .read()
                    .map_err(|_| "interaction handler lock poisoned".to_string())
                    .and_then(|guard| {
                        guard
                            .clone()
                            .ok_or_else(|| "approval requires an interaction handler".to_string())
                    })
                    .and_then(|handler| {
                        handler.approve(ApprovalRequest {
                            token,
                            request_id: approve_id.clone(),
                            tool: tool.to_string(),
                            detail: detail.to_string(),
                        })
                    });
                match result {
                    Ok(approved) => approved,
                    Err(error) => {
                        if let Ok(mut slot) = approve_error.lock() {
                            *slot = Some(error);
                        }
                        false
                    }
                }
            }),
            goal_event: Some(Arc::new(move |text: &str, status: &str| {
                // Best-effort: a background goal-lifecycle event never fails
                // or outlives the prompt's error handling.
                let _ = goal_inner.emit(SessionEvent {
                    request_id: goal_id.clone(),
                    event: SessionEventKind::Goal {
                        text: text.to_string(),
                        status: status.to_string(),
                    },
                });
            })),
        };
        let result = if continuing {
            conversation.continue_work(&args, &mut hooks)
        } else if request.prompt.split_whitespace().next() == Some("/todo") {
            let mut parts =
                shell_words::split(&request.prompt).map_err(|error| error.to_string())?;
            parts.remove(usize::default());
            if parts.first().map(String::as_str) == Some("continue") {
                conversation.continue_work(&args, &mut hooks)
            } else {
                parts.extend([
                    "--session".into(),
                    conversation.session_path().display().to_string(),
                ]);
                crate::completion::cli::execute(&args.cwd, &parts, false, Some(&args))
            }
        } else {
            conversation.run_turn(&args, &request.prompt, &[], &mut hooks)
        };
        *self
            .inner
            .session_path
            .write()
            .map_err(|_| "session path lock poisoned")? = conversation.session_path();
        let text = result?;
        // The filter holds the last unterminated line back until it knows the
        // line is not a fence; the turn is over, so let it out.
        if let Some(tail) = code_filter.borrow_mut().as_mut().map(CodeFilter::finish) {
            if !tail.is_empty() {
                self.inner.emit(SessionEvent {
                    request_id: request_id.clone(),
                    event: SessionEventKind::TextDelta { text: tail },
                })?;
            }
        }
        if let Some(error) = event_error
            .lock()
            .map_err(|_| "event error lock poisoned")?
            .take()
        {
            return Err(error);
        }
        let text = policy.answer(text);
        let session_path = conversation.session_path();
        let completion = conversation.completion_state()?;
        self.inner.emit(SessionEvent {
            request_id: request_id.clone(),
            event: SessionEventKind::Result {
                text: text.clone(),
                completion: completion.clone(),
            },
        })?;
        Ok(PromptResult {
            request_id,
            text,
            session_path,
            completion,
        })
    }
}
fn args_from_options(options: &SessionOptions, prompt: String, goal: Option<String>) -> Args {
    Args {
        command: "run".into(),
        cwd: options.cwd.clone(),
        cwd_explicit: true,
        model: options.model.clone(),
        max_tokens: options.max_tokens,
        max_steps: options.max_steps,
        allow_write: options.allow_write || options.auto_approve,
        allow_command: options.allow_command || options.auto_approve,
        yolo: options.auto_approve,
        model_only: false,
        json: false,
        resume_session: None,
        autonomous: false,
        goal,
        positionals: vec![prompt],
    }
}
