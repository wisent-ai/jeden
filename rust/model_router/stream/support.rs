use super::*;

pub(crate) fn malformed(message: impl Into<String>, visible_output: bool) -> AttemptError {
    AttemptError {
        class: StreamErrorClass::MalformedEvent,
        message: message.into(),
        retry_after: None,
        visible_output,
    }
}

/// A successful-but-empty router response is transient: retrying the same
/// request can legitimately return content.
pub(crate) fn empty_response(message: impl Into<String>, visible_output: bool) -> AttemptError {
    AttemptError {
        class: StreamErrorClass::EmptyResponse,
        message: message.into(),
        retry_after: None,
        visible_output,
    }
}

pub(crate) fn http_error(status: u16, body: String, retry_after: Option<Duration>) -> AttemptError {
    let normalized = body.to_ascii_lowercase();
    let contract = serde_json::from_str::<Value>(&body).ok();
    let declared_retryable = contract
        .as_ref()
        .and_then(|value| value.pointer("/error/retryable"))
        .and_then(Value::as_bool);
    let error_code = contract
        .as_ref()
        .and_then(|value| value.pointer("/error/code"))
        .and_then(Value::as_str);
    // A 429 `subscription_unavailable` fires while a reauth is still in
    // progress; it clears on its own, so treat it as transient rather than
    // quota exhaustion.
    let subscription_transient = status == 429 && normalized.contains("subscription_unavailable");
    let quota_exhausted = error_code == Some("provider_quota_exhausted")
        || status == 402
        || (status == 429 && (normalized.contains("quota") || normalized.contains("subscription")));
    // An explicit `"retryable": false` is the gateway answering the question
    // this function guesses at from the status family. It wins: a subscription
    // the provider rejected needs a human to authorize it again, and retrying
    // spends the session's budget on a wait that cannot end.
    let refused_outright =
        normalized.contains("\"retryable\":false") || normalized.contains("\"retryable\": false");
    let class = if refused_outright {
        StreamErrorClass::Permanent
    } else if subscription_transient {
        StreamErrorClass::TransientHttp
    } else if quota_exhausted {
        StreamErrorClass::QuotaExhausted
    } else if declared_retryable == Some(false) {
        StreamErrorClass::Permanent
    } else if matches!(status, 408 | 409 | 425 | 429) || (500..600).contains(&status) {
        StreamErrorClass::TransientHttp
    } else if is_context_overflow_body(&body) {
        StreamErrorClass::ContextOverflow
    } else {
        StreamErrorClass::Permanent
    };
    AttemptError {
        class,
        message: format!(
            "model router {status}: {}",
            body.chars().take(800).collect::<String>()
        ),
        retry_after,
        visible_output: false,
    }
}

pub(crate) fn is_context_overflow_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("context length")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("too many tokens")
        || lower.contains("tokens exceed")
}

/// Take the next message from the stream adapter. It arrives, the adapter
/// disconnects, or the operator cancels the turn; a model that is still
/// thinking is not one of those, which is what the old first-event and idle
/// deadlines used to report it as.
pub(crate) fn recv_until(
    receiver: &Receiver<WireMessage>,
    cancelled: &dyn Fn() -> bool,
) -> Result<WireMessage, StreamErrorClass> {
    loop {
        if cancelled() {
            return Err(StreamErrorClass::Cancelled);
        }
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(message) => return Ok(message),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(StreamErrorClass::Network),
        }
    }
}

pub(crate) fn retry_delay(policy: &RetryPolicy, attempt: usize, retry_after: Option<Duration>) -> Duration {
    if let Some(delay) = retry_after {
        return delay;
    }
    // With the default 2s base this backs off ~2s then ~8s (capped), giving a
    // recovering router real time instead of hammering it.
    let exponent = attempt.saturating_sub(1).saturating_mul(2).min(20) as u32;
    let base_ms = policy
        .base_delay
        .as_millis()
        .saturating_mul(1u128 << exponent);
    let capped_ms = base_ms.min(policy.max_delay.as_millis()) as f64;
    let jitter = policy.jitter_ratio.clamp(0.0, 1.0);
    let factor = rand::thread_rng().gen_range((1.0 - jitter)..=(1.0 + jitter));
    Duration::from_millis((capped_ms * factor).round().max(0.0) as u64)
}

pub(crate) fn cancellable_sleep(delay: Duration, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline {
        if cancelled() {
            return Err("Turn cancelled.".into());
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(25)),
        );
    }
    Ok(())
}

pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let mut fields = value.split_whitespace();
    let weekday = fields.next()?;
    if !weekday.ends_with(',') {
        return None;
    }
    let day = fields.next()?.parse::<u32>().ok()?;
    let month = match fields.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = fields.next()?.parse::<i64>().ok()?;
    let mut clock = fields.next()?.split(':');
    let hour = clock.next()?.parse::<u32>().ok()?;
    let minute = clock.next()?.parse::<u32>().ok()?;
    let second = clock.next()?.parse::<u32>().ok()?;
    if clock.next().is_some()
        || fields.next()? != "GMT"
        || fields.next().is_some()
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let target = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let target = u64::try_from(target).ok()?;
    Some(Duration::from_secs(target.saturating_sub(now)))
}

pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(crate) struct DigestSink<'a>(&'a mut Sha256);

impl std::io::Write for DigestSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn logical_request_keys(route: &RouteDescriptor, messages: &[Value]) -> (String, String, String) {
    let mut hasher = Sha256::new();
    if serde_json::to_writer(DigestSink(&mut hasher), &(route, messages)).is_err() {
        hasher = Sha256::new();
    }
    let digest = hex::encode(hasher.finalize());
    (
        format!("request-{digest}"),
        format!("completion-{digest}"),
        format!("session-{digest}"),
    )
}

pub(crate) fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub(crate) fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

pub(crate) fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

/// Merge streaming tool-call deltas (indexed) into a growing list.
pub(crate) fn accumulate_tool_call_deltas(acc: &mut Vec<Value>, deltas: &[Value]) -> Result<(), String> {
    for delta in deltas {
        let raw_index = delta.get("index").and_then(Value::as_u64).unwrap_or(0);
        let index =
            usize::try_from(raw_index).map_err(|_| "tool call index exceeds platform size")?;
        if index >= MAX_TOOL_CALLS {
            return Err(format!(
                "tool call index {index} exceeds limit {MAX_TOOL_CALLS}"
            ));
        }
        while acc.len() <= index {
            acc.push(json!({"function": {"name": "", "arguments": ""}}));
        }
        let slot = &mut acc[index];
        if let Some(name) = delta.pointer("/function/name").and_then(Value::as_str) {
            if !name.is_empty() {
                let current = slot
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let updated = format!("{current}{name}");
                slot["function"]["name"] = Value::String(updated);
            }
        }
        if let Some(arguments) = delta.pointer("/function/arguments").and_then(Value::as_str) {
            let current = slot
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("");
            let updated = format!("{current}{arguments}");
            slot["function"]["arguments"] = Value::String(updated);
        }
    }
    Ok(())
}
