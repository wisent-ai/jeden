use super::client::{ClientError, SessionClient, SessionTransport, TransportError};
use super::protocol::{
    Envelope, ErrorEnvelope, EventEnvelope, ProtocolErrorBody, RequestEnvelope, RequestMeta,
    ResponseEnvelope,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_stream::StreamExt;

struct ChannelTransport {
    sent: mpsc::UnboundedSender<Envelope>,
    incoming: Mutex<mpsc::UnboundedReceiver<Result<Envelope, TransportError>>>,
}

impl SessionTransport for ChannelTransport {
    fn send(&self, envelope: Envelope) -> BoxFuture<'_, Result<(), TransportError>> {
        Box::pin(async move {
            self.sent
                .send(envelope)
                .map_err(|_| TransportError::new("test outbound channel closed"))
        })
    }

    fn recv(&self) -> BoxFuture<'_, Result<Envelope, TransportError>> {
        Box::pin(async move {
            self.incoming
                .lock()
                .await
                .recv()
                .await
                .unwrap_or_else(|| Err(TransportError::new("test inbound channel closed")))
        })
    }
}

struct Harness {
    client: SessionClient,
    sent: mpsc::UnboundedReceiver<Envelope>,
    incoming: mpsc::UnboundedSender<Result<Envelope, TransportError>>,
}

fn harness() -> Harness {
    let (sent_tx, sent_rx) = mpsc::unbounded_channel();
    let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
    let transport = Arc::new(ChannelTransport {
        sent: sent_tx,
        incoming: Mutex::new(incoming_rx),
    });
    Harness {
        client: SessionClient::new(transport),
        sent: sent_rx,
        incoming: incoming_tx,
    }
}

async fn next_request(sent: &mut mpsc::UnboundedReceiver<Envelope>) -> RequestEnvelope {
    match sent.recv().await.expect("client must send an envelope") {
        Envelope::Request(request) => request,
        other => panic!("expected request envelope, got {other:?}"),
    }
}

fn response(id: &str, result: Value) -> Envelope {
    Envelope::Response(ResponseEnvelope::new(id, result).expect("valid response"))
}

fn event(sequence: u64, cursor: &str, request_id: Option<&str>) -> Envelope {
    Envelope::Event(
        EventEnvelope::new(
            "session-1",
            "main",
            sequence,
            cursor,
            format!("event-{sequence}"),
            request_id.map(str::to_owned),
            "message.delta",
            json!({ "part": sequence }),
        )
        .expect("valid event"),
    )
}

#[test]
fn canonical_golden_envelopes_remain_wire_identical() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protocol/schema/v1/golden/envelopes.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let fixtures: Vec<Value> = serde_json::from_str(&text).expect("golden fixture must be JSON");

    for fixture in fixtures {
        let envelope: Envelope =
            serde_json::from_value(fixture.clone()).expect("golden envelope must deserialize");
        envelope.validate().expect("golden envelope must validate");
        assert_eq!(serde_json::to_value(envelope).unwrap(), fixture);
    }
}

#[tokio::test]
async fn concurrent_out_of_order_responses_are_correlated_by_id() {
    let mut harness = harness();
    let first_client = harness.client.clone();
    let second_client = harness.client.clone();
    let first = tokio::spawn(async move {
        first_client
            .call(
                "request-1",
                "session.read",
                json!({ "slot": 1 }),
                RequestMeta::new("idem-1"),
            )
            .await
    });
    let second = tokio::spawn(async move {
        second_client
            .call(
                "request-2",
                "session.read",
                json!({ "slot": 2 }),
                RequestMeta::new("idem-2"),
            )
            .await
    });

    let outbound_a = next_request(&mut harness.sent).await;
    let outbound_b = next_request(&mut harness.sent).await;
    assert_ne!(outbound_a.id, outbound_b.id);

    harness
        .incoming
        .send(Ok(response("request-2", json!({ "answer": 2 }))))
        .unwrap();
    harness
        .incoming
        .send(Ok(response("request-1", json!({ "answer": 1 }))))
        .unwrap();

    assert_eq!(first.await.unwrap().unwrap(), json!({ "answer": 1 }));
    assert_eq!(second.await.unwrap().unwrap(), json!({ "answer": 2 }));
    harness.client.dispose().await;
}

#[tokio::test]
async fn correlated_protocol_error_preserves_typed_body() {
    let mut harness = harness();
    let client = harness.client.clone();
    let pending = tokio::spawn(async move {
        client
            .call(
                "request-error",
                "session.read",
                json!({}),
                RequestMeta::new("idem-error"),
            )
            .await
    });
    let request = next_request(&mut harness.sent).await;
    assert_eq!(request.id, "request-error");

    let body = ProtocolErrorBody::new(
        "session.missing",
        "session was not found",
        false,
        json!({ "sessionId": "session-1" }),
    )
    .unwrap();
    harness
        .incoming
        .send(Ok(Envelope::Error(
            ErrorEnvelope::new(Some(request.id), body).unwrap(),
        )))
        .unwrap();

    match pending.await.unwrap().unwrap_err() {
        ClientError::Protocol(error) => {
            assert_eq!(error.id.as_deref(), Some("request-error"));
            assert_eq!(error.error.code, "session.missing");
            assert_eq!(error.error.message, "session was not found");
            assert!(!error.error.retryable);
            assert_eq!(error.error.details, json!({ "sessionId": "session-1" }));
        }
        other => panic!("expected typed protocol error, got {other:?}"),
    }
    harness.client.dispose().await;
}

#[tokio::test]
async fn events_retain_arrival_order_and_canonical_stream_fields() {
    let harness = harness();
    let mut events = harness.client.events();
    for (sequence, cursor, request_id) in [
        (7, "cursor-7", Some("request-7")),
        (8, "cursor-8", None),
        (9, "cursor-9", Some("request-9")),
    ] {
        harness
            .incoming
            .send(Ok(event(sequence, cursor, request_id)))
            .unwrap();
    }

    for (sequence, cursor, request_id) in [
        (7, "cursor-7", Some("request-7")),
        (8, "cursor-8", None),
        (9, "cursor-9", Some("request-9")),
    ] {
        let received = events.next().await.unwrap().unwrap();
        assert_eq!(received.sequence, sequence);
        assert_eq!(received.cursor, cursor);
        assert_eq!(received.request_id.as_deref(), request_id);
        assert_eq!(received.session_id, "session-1");
        assert_eq!(received.stream_id, "main");
    }
    assert_eq!(
        harness.client.last_cursor("session-1").as_deref(),
        Some("cursor-9")
    );
    harness.client.dispose().await;
}

#[tokio::test]
async fn replay_and_reconnect_emit_canonical_cursor_fields() {
    let mut harness = harness();
    let replay_client = harness.client.clone();
    let replay = tokio::spawn(async move {
        replay_client
            .replay(
                "session-1",
                Some("cursor-explicit".to_owned()),
                Some(25),
                RequestMeta::new("idem-replay"),
            )
            .await
    });
    let replay_request = next_request(&mut harness.sent).await;
    assert_eq!(replay_request.method, "session.replay");
    assert_eq!(
        replay_request.params,
        json!({ "sessionId": "session-1", "cursor": "cursor-explicit", "limit": 25 })
    );
    assert_eq!(replay_request.meta.idempotency_key, "idem-replay");
    harness
        .incoming
        .send(Ok(response(
            &replay_request.id,
            json!({ "accepted": true }),
        )))
        .unwrap();
    assert_eq!(replay.await.unwrap().unwrap(), json!({ "accepted": true }));

    let mut events = harness.client.events();
    harness
        .incoming
        .send(Ok(event(26, "cursor-observed", None)))
        .unwrap();
    assert_eq!(
        events.next().await.unwrap().unwrap().cursor,
        "cursor-observed"
    );

    let reconnect_client = harness.client.clone();
    let reconnect = tokio::spawn(async move {
        reconnect_client
            .reconnect("session-1", Some(10), RequestMeta::new("idem-reconnect"))
            .await
    });
    let reconnect_request = next_request(&mut harness.sent).await;
    assert_eq!(reconnect_request.method, "session.replay");
    assert_eq!(
        reconnect_request.params,
        json!({ "sessionId": "session-1", "cursor": "cursor-observed", "limit": 10 })
    );
    assert_eq!(reconnect_request.meta.idempotency_key, "idem-reconnect");
    harness
        .incoming
        .send(Ok(response(&reconnect_request.id, json!(null))))
        .unwrap();
    assert_eq!(reconnect.await.unwrap().unwrap(), Value::Null);
    harness.client.dispose().await;
}

#[tokio::test]
async fn cancel_requires_and_forwards_caller_idempotency() {
    let mut harness = harness();
    match harness
        .client
        .cancel("target-request", RequestMeta::new(""))
        .await
        .unwrap_err()
    {
        ClientError::Validation(error) => assert_eq!(error.field, "meta.idempotencyKey"),
        other => panic!("expected idempotency validation error, got {other:?}"),
    }
    assert!(
        harness.sent.try_recv().is_err(),
        "invalid cancel must not be sent"
    );

    let cancel_client = harness.client.clone();
    let cancel = tokio::spawn(async move {
        cancel_client
            .cancel("target-request", RequestMeta::new("cancel-idempotency-key"))
            .await
    });
    let request = next_request(&mut harness.sent).await;
    assert_eq!(request.method, "request.cancel");
    assert_eq!(request.params, json!({ "requestId": "target-request" }));
    assert_eq!(request.meta.idempotency_key, "cancel-idempotency-key");
    harness
        .incoming
        .send(Ok(response(&request.id, json!({ "cancelled": true }))))
        .unwrap();
    assert_eq!(cancel.await.unwrap().unwrap(), json!({ "cancelled": true }));
    harness.client.dispose().await;
}

#[tokio::test]
async fn duplicate_in_flight_request_id_is_denied_without_second_send() {
    let mut harness = harness();
    let first_client = harness.client.clone();
    let first = tokio::spawn(async move {
        first_client
            .call(
                "duplicate",
                "session.read",
                json!({}),
                RequestMeta::new("idem-first"),
            )
            .await
    });
    let outbound = next_request(&mut harness.sent).await;

    match harness
        .client
        .call(
            "duplicate",
            "session.read",
            json!({}),
            RequestMeta::new("idem-second"),
        )
        .await
        .unwrap_err()
    {
        ClientError::DuplicateRequestId(id) => assert_eq!(id, "duplicate"),
        other => panic!("expected duplicate request denial, got {other:?}"),
    }
    assert!(
        harness.sent.try_recv().is_err(),
        "duplicate must not be sent"
    );

    harness
        .incoming
        .send(Ok(response(&outbound.id, json!("first-result"))))
        .unwrap();
    assert_eq!(first.await.unwrap().unwrap(), json!("first-result"));
    harness.client.dispose().await;
}

#[tokio::test]
async fn transport_failure_fails_all_pending_requests_and_event_stream() {
    let mut harness = harness();
    let mut events = harness.client.events();
    let first_client = harness.client.clone();
    let second_client = harness.client.clone();
    let first = tokio::spawn(async move {
        first_client
            .call(
                "pending-1",
                "session.read",
                json!({}),
                RequestMeta::new("idem-1"),
            )
            .await
    });
    let second = tokio::spawn(async move {
        second_client
            .call(
                "pending-2",
                "session.read",
                json!({}),
                RequestMeta::new("idem-2"),
            )
            .await
    });
    let _ = next_request(&mut harness.sent).await;
    let _ = next_request(&mut harness.sent).await;

    harness
        .incoming
        .send(Err(TransportError::new("connection reset")))
        .unwrap();

    for result in [first.await.unwrap(), second.await.unwrap()] {
        match result.unwrap_err() {
            ClientError::Transport(error) => assert_eq!(error.message(), "connection reset"),
            other => panic!("expected transport error, got {other:?}"),
        }
    }
    match events.next().await.unwrap().unwrap_err() {
        ClientError::Transport(error) => assert_eq!(error.message(), "connection reset"),
        other => panic!("expected terminal stream transport error, got {other:?}"),
    }
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn dispose_terminates_event_stream_and_denies_new_calls() {
    let harness = harness();
    let mut events = harness.client.events();
    harness.client.dispose().await;

    assert!(matches!(
        events.next().await,
        Some(Err(ClientError::Disposed))
    ));
    assert!(events.next().await.is_none());
    assert!(matches!(
        harness
            .client
            .call(
                "after-dispose",
                "session.read",
                json!({}),
                RequestMeta::new("idem")
            )
            .await,
        Err(ClientError::Disposed)
    ));
}
