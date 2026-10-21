use super::*;

/// HTTP 402, which a router answers when the provider's quota is spent
/// (https://www.rfc-editor.org/rfc/rfc9110#section-15.5.3).
const PAYMENT_REQUIRED: u16 = reqwest::StatusCode::PAYMENT_REQUIRED.as_u16();
/// HTTP 409 (https://www.rfc-editor.org/rfc/rfc9110#section-15.5.10).
const CONFLICT: u16 = reqwest::StatusCode::CONFLICT.as_u16();
/// HTTP 425 (https://www.rfc-editor.org/rfc/rfc8470#section-5.2).
const TOO_EARLY: u16 = reqwest::StatusCode::TOO_EARLY.as_u16();
/// HTTP 429 (https://www.rfc-editor.org/rfc/rfc6585#section-4).
const TOO_MANY_REQUESTS: u16 = reqwest::StatusCode::TOO_MANY_REQUESTS.as_u16();

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
    // Brama answers every refusal with its contract document; the class is
    // read from its fields, never from words in the body. A body that is not
    // JSON carries none of them, and the message below carries it whole.
    let contract = serde_json::from_str::<Value>(&body);
    let declared_retryable = contract
        .iter()
        .find_map(|value| value.pointer("/error/retryable"))
        .and_then(Value::as_bool);
    let error_code = contract
        .iter()
        .find_map(|value| value.pointer("/error/code"))
        .and_then(Value::as_str);
    // A 429 `subscription_unavailable` fires while a reauth is still in
    // progress; it clears on its own, so treat it as transient rather than
    // quota exhaustion.
    let subscription_transient = error_code == Some("subscription_unavailable");
    let quota_exhausted =
        error_code == Some("provider_quota_exhausted") || status == PAYMENT_REQUIRED;
    // A prompt longer than the model takes is refused with `retryable: false`
    // too, but it has its own repair: a larger route or a compacted context.
    // Brama names it `context_length_exceeded` from the provider's status or
    // error code; the words of the body decide nothing.
    // An explicit `retryable` is otherwise the gateway answering the question
    // this function guesses at from the status family, and it wins either way:
    // a subscription the provider rejected needs a human to authorize it
    // again, and retrying spends the session's budget on a wait that cannot
    // end, while a refusal the gateway calls retryable is retried whatever
    // its status.
    let server_error = reqwest::StatusCode::from_u16(status)
        .as_ref()
        .is_ok_and(reqwest::StatusCode::is_server_error);
    let class = if error_code == Some("context_length_exceeded") {
        StreamErrorClass::ContextOverflow
    } else if declared_retryable == Some(false) {
        StreamErrorClass::Permanent
    } else if subscription_transient || declared_retryable == Some(true) {
        StreamErrorClass::TransientHttp
    } else if quota_exhausted {
        StreamErrorClass::QuotaExhausted
    } else if matches!(status, CONFLICT | TOO_EARLY | TOO_MANY_REQUESTS) || server_error {
        StreamErrorClass::TransientHttp
    } else {
        StreamErrorClass::Permanent
    };
    AttemptError {
        class,
        message: format!("model router {status}: {body}"),
        retry_after,
        visible_output: false,
    }
}

/// Take the next message from the stream adapter. It arrives, the adapter
/// disconnects, or the operator cancels the turn; a model that is still
/// thinking is not one of those. The receive blocks: a cancellation reaches
/// it as `WireMessage::Cancelled`, sent by the attempt's cancellation wake,
/// and is acted on only when it is this turn's flag that is set. A channel
/// whose sender is gone is the adapter disconnecting.
pub(crate) fn recv_until(
    receiver: &Receiver<WireMessage>,
    cancelled: &dyn Fn() -> bool,
) -> Result<WireMessage, StreamErrorClass> {
    loop {
        if cancelled() {
            return Err(StreamErrorClass::Cancelled);
        }
        match receiver.recv() {
            Ok(WireMessage::Cancelled) => continue,
            Ok(message) => return Ok(message),
            Err(mpsc::RecvError) => return Err(StreamErrorClass::Network),
        }
    }
}

/// `Retry-After` in its delay-seconds form
/// (https://www.rfc-editor.org/rfc/rfc9110#section-10.2.3), the form Brama
/// and the providers Jeden reaches answer with. The HTTP-date form is refused
/// by name rather than read as no wait: telling it would need the month names
/// as a word list, and reading the clock against it would turn a clock set
/// before the Unix epoch into no wait at all.
pub(crate) fn parse_retry_after(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    value
        .parse::<u64>()
        .map(Duration::from_secs)
        .map_err(|error| {
            format!("Retry-After {value:?} is not delay-seconds, the form Jeden reads: {error}")
        })
}

pub(crate) fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

/// One function field of an accumulated tool call: what the stream has
/// given so far. A slot is created with both fields, so one that lacks a
/// field is refused naming it.
fn accumulated<'a>(slot: &'a Value, field: &str) -> Result<&'a str, String> {
    slot.pointer(&format!("/function/{field}"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("accumulated tool call carries no function {field}: {slot}"))
}

/// Merge streaming tool-call deltas (indexed) into a growing list.
pub(crate) fn accumulate_tool_call_deltas(
    acc: &mut Vec<Value>,
    deltas: &[Value],
) -> Result<(), String> {
    for delta in deltas {
        let raw_index = delta
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("tool call delta carries no index: {delta}"))?;
        let index = usize::try_from(raw_index).map_err(|error| {
            format!("tool call index {raw_index} exceeds platform size: {error}")
        })?;
        if index > acc.len() {
            return Err(format!(
                "tool call index {index} skips past the {} calls opened so far; a stream opens tool calls in order",
                acc.len()
            ));
        }
        if index == acc.len() {
            acc.push(json!({"function": {"name": "", "arguments": ""}}));
        }
        let slot = &mut acc[index];
        if let Some(name) = delta.pointer("/function/name").and_then(Value::as_str) {
            if !name.is_empty() {
                let updated = format!("{}{name}", accumulated(slot, "name")?);
                slot["function"]["name"] = Value::String(updated);
            }
        }
        if let Some(arguments) = delta.pointer("/function/arguments").and_then(Value::as_str) {
            let updated = format!("{}{arguments}", accumulated(slot, "arguments")?);
            slot["function"]["arguments"] = Value::String(updated);
        }
    }
    Ok(())
}
