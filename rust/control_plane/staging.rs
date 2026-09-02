use super::brama::BramaClient;
use super::contract::{
    BramaApiV1, ContractError, ModelRequest, RequestMeta, RouteRequest, WelesApiV1,
};
use super::transport::{ReqwestTransport, SecretRef};
use super::weles::WelesClient;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

const REQUIRED_ENV: &[(&str, &str)] = &[
    ("BRAMA_STAGING_URL", "Brama staging HTTPS endpoint"),
    ("WELES_STAGING_URL", "Weles staging HTTPS endpoint"),
    (
        "JEDEN_STAGING_OIDC_TOKEN",
        "short-lived workload OIDC credential for the configured audience/role",
    ),
    ("JEDEN_STAGING_OIDC_AUDIENCE", "workload OIDC audience"),
    ("JEDEN_STAGING_OIDC_ROLE", "staging workload role"),
    (
        "JEDEN_STAGING_TENANT",
        "disposable staging tenant/account namespace",
    ),
    (
        "JEDEN_STAGING_PROVIDER",
        "provider enabled for disposable lifecycle",
    ),
    ("JEDEN_STAGING_MODEL", "harmless model route with quota"),
    (
        "JEDEN_STAGING_SCHEMA_MIN",
        "minimum supported staging schema version",
    ),
    (
        "JEDEN_STAGING_SCHEMA_MAX",
        "maximum supported staging schema version",
    ),
    (
        "JEDEN_STAGING_REPORT_SIGNING_KEY_HEX",
        "32-byte short-lived Ed25519 report signing seed",
    ),
    (
        "JEDEN_RELEASE_DIGEST",
        "immutable released canary digest under certification",
    ),
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StagingEvidence {
    pub schema_version: u32,
    pub status: String,
    pub release_digest: String,
    pub endpoint_identities: Vec<String>,
    pub schema_revisions: Vec<String>,
    pub request_ids: Vec<String>,
    pub operation_ids: Vec<String>,
    pub served_route: String,
    pub usage_input_tokens: u64,
    pub usage_output_tokens: u64,
    pub redacted_trace_refs: Vec<String>,
    pub evidence_digest: String,
    pub signing_public_key: String,
    pub signing_key_id: String,
    pub signature: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsignedEvidence<'a> {
    schema_version: u32,
    status: &'a str,
    release_digest: &'a str,
    endpoint_identities: &'a [String],
    schema_revisions: &'a [String],
    request_ids: &'a [String],
    operation_ids: &'a [String],
    served_route: &'a str,
    usage_input_tokens: u64,
    usage_output_tokens: u64,
    redacted_trace_refs: &'a [String],
    signing_key_id: &'a str,
    signing_public_key: &'a str,
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

fn identity(endpoint: &str) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(endpoint.as_bytes()))
    )
}

pub fn staging_preflight_from_env() -> Result<(BramaClient, WelesClient), ContractError> {
    let prerequisites = REQUIRED_ENV
        .iter()
        .filter(|&(name, _detail)| required(name)
                .is_empty()).map(|(name, detail)| format!("{name}: {detail}"))
        .collect::<Vec<_>>();
    if !prerequisites.is_empty() {
        return Err(ContractError::ExternalBlocked { prerequisites });
    }
    let brama = BramaClient::with_secret_ref(
        Some(required("BRAMA_STAGING_URL")),
        Some(SecretRef::environment("JEDEN_STAGING_OIDC_TOKEN")),
        Duration::from_secs(30),
        ReqwestTransport::production(),
    );
    let weles = WelesClient::with_secret_ref(
        Some(required("WELES_STAGING_URL")),
        Some(SecretRef::environment("JEDEN_STAGING_OIDC_TOKEN")),
        Duration::from_millis(500),
        ReqwestTransport::production(),
    );
    Ok((brama, weles))
}

fn poll_to_terminal(
    client: &WelesClient,
    mut operation: super::weles::OperationV1,
    correlation_id: &str,
    schema_min: u32,
    schema_max: u32,
) -> Result<super::weles::OperationV1, String> {
    for _ in 0..256 {
        if !matches!(operation.state.as_str(), "pending" | "running") {
            return Ok(operation);
        }
        operation = WelesApiV1::poll_operation(
            client,
            &operation.id,
            operation.cursor.as_deref(),
            &RequestMeta {
                correlation_id: correlation_id.into(),
                idempotency_key: None,
                schema_min,
                schema_max,
            },
        )
        .map_err(|error| error.to_string())?;
    }
    Err("Weles operation exceeded 256 cursor pages".into())
}

pub fn run_staging_readiness() -> Result<StagingEvidence, String> {
    let (brama, weles) = staging_preflight_from_env().map_err(|error| format!("{error:?}"))?;
    let schema_min = required("JEDEN_STAGING_SCHEMA_MIN")
        .parse::<u32>()
        .map_err(|_| "JEDEN_STAGING_SCHEMA_MIN must be u32".to_string())?;
    let schema_max = required("JEDEN_STAGING_SCHEMA_MAX")
        .parse::<u32>()
        .map_err(|_| "JEDEN_STAGING_SCHEMA_MAX must be u32".to_string())?;
    super::contract::negotiate(schema_min, schema_max).map_err(|error| format!("{error:?}"))?;

    let brama_health = BramaApiV1::health(&brama);
    let weles_health = WelesApiV1::health(&weles);
    let brama_ready = BramaApiV1::readiness(&brama).map_err(|error| error.to_string())?;
    let weles_ready = WelesApiV1::readiness(&weles).map_err(|error| error.to_string())?;
    if !brama_ready.ready || !weles_ready.ready {
        return Err("control plane readiness returned not-ready".into());
    }

    let catalog = BramaApiV1::catalog(&brama, true).map_err(|error| error.to_string())?;
    let model = required("JEDEN_STAGING_MODEL");
    let request_ids = (0..8)
        .map(|index| format!("staging-{}-{index}", super::now_ms()))
        .collect::<Vec<_>>();
    let read_meta = RequestMeta {
        correlation_id: request_ids[0].clone(),
        idempotency_key: None,
        schema_min,
        schema_max,
    };
    let _capabilities =
        BramaApiV1::capabilities(&brama, &read_meta).map_err(|error| error.to_string())?;
    let route = BramaApiV1::resolve(
        &brama,
        &RouteRequest {
            model: model.clone(),
            required_modalities: vec!["text".into()],
            requires_tools: false,
        },
        &RequestMeta {
            correlation_id: request_ids[1].clone(),
            idempotency_key: Some(request_ids[1].clone()),
            schema_min,
            schema_max,
        },
    )
    .map_err(|error| error.to_string())?;
    let stream = BramaApiV1::stream(
        &brama,
        &ModelRequest {
            route: route.id.clone(),
            prompt: "Reply with exactly: staging-ok".into(),
            max_output_tokens: 16,
        },
        &RequestMeta {
            correlation_id: request_ids[2].clone(),
            idempotency_key: Some(request_ids[2].clone()),
            schema_min,
            schema_max,
        },
        &|| false,
    )
    .map_err(|error| error.to_string())?;

    let provider = required("JEDEN_STAGING_PROVIDER");
    let tenant = required("JEDEN_STAGING_TENANT");
    let mutation = |index: usize| RequestMeta {
        correlation_id: request_ids[index].clone(),
        idempotency_key: Some(format!("{}:{}", tenant, request_ids[index])),
        schema_min,
        schema_max,
    };
    let login = WelesApiV1::begin_login(
        &weles,
        &provider,
        &format!("jeden:staging:{tenant}"),
        &mutation(3),
    )
    .map_err(|error| error.to_string())?;
    let login_id = login.id.clone();
    let operation = poll_to_terminal(&weles, login, &request_ids[4], schema_min, schema_max)?;
    if operation
        .events
        .iter()
        .any(|event| matches!(event, super::weles::OperationEvent::Elicit { .. }))
    {
        let _ = WelesApiV1::cancel_operation(&weles, &login_id, &mutation(7));
        return Err(
            "staging disposable login unexpectedly requires interactive secret input".into(),
        );
    }
    if operation.state != "completed" {
        return Err(format!("disposable login ended in {}", operation.state));
    }
    let account_id = operation
        .events
        .iter()
        .find_map(|event| match event {
            super::weles::OperationEvent::Completed { account } => {
                account.as_ref().map(|account| account.id.clone())
            }
            _ => None,
        })
        .ok_or_else(|| "completed login omitted disposable account".to_string())?;

    let refresh_start =
        WelesApiV1::refresh(&weles, &account_id, &mutation(5)).map_err(|error| error.to_string());
    let refresh_id = refresh_start
        .as_ref()
        .map(|operation| operation.id.clone())
        .unwrap_or_default();
    let refresh_result = refresh_start.and_then(|operation| {
        poll_to_terminal(&weles, operation, &request_ids[5], schema_min, schema_max)
    });

    // Cleanup is deliberately attempted regardless of refresh outcome.
    let logout_start =
        WelesApiV1::logout(&weles, &account_id, &mutation(6)).map_err(|error| error.to_string());
    let logout_id = logout_start
        .as_ref()
        .map(|operation| operation.id.clone())
        .unwrap_or_default();
    let logout_result = logout_start.and_then(|operation| {
        poll_to_terminal(&weles, operation, &request_ids[6], schema_min, schema_max)
    });
    let refresh = refresh_result?;
    let logout = logout_result?;
    if refresh.state != "completed" || logout.state != "completed" {
        return Err(format!(
            "refresh/logout terminal states were {}/{}",
            refresh.state, logout.state
        ));
    }
    let operation_ids = vec![login_id, refresh_id, logout_id];

    let endpoint_identities = [
        brama_health.endpoint.as_deref(),
        weles_health.endpoint.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(identity)
    .collect::<Vec<_>>();
    let schema_revisions = vec![
        catalog.version,
        catalog.catalog_revision,
        format!("{}-{}", brama_ready.schema_min, brama_ready.schema_max),
        format!("{}-{}", weles_ready.schema_min, weles_ready.schema_max),
    ];
    let redacted_trace_refs = request_ids
        .iter()
        .map(|id| identity(id))
        .collect::<Vec<_>>();
    let release_digest = required("JEDEN_RELEASE_DIGEST");
    let seed = hex::decode(required("JEDEN_STAGING_REPORT_SIGNING_KEY_HEX"))
        .map_err(|_| "report signing seed must be hex".to_string())?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| "report signing seed must be exactly 32 bytes".to_string())?;
    let signing = SigningKey::from_bytes(&seed);
    let signing_public_key = hex::encode(signing.verifying_key().as_bytes());
    let signing_key_id = format!(
        "ed25519:{}",
        hex::encode(Sha256::digest(signing.verifying_key().as_bytes()))
    );
    let unsigned = UnsignedEvidence {
        schema_version: 1,
        status: "passed",
        release_digest: &release_digest,
        endpoint_identities: &endpoint_identities,
        schema_revisions: &schema_revisions,
        request_ids: &request_ids,
        operation_ids: &operation_ids,
        served_route: &stream.served_route,
        usage_input_tokens: stream.usage.input_tokens,
        usage_output_tokens: stream.usage.output_tokens,
        redacted_trace_refs: &redacted_trace_refs,
        signing_key_id: &signing_key_id,
        signing_public_key: &signing_public_key,
    };
    let canonical = serde_json::to_vec(&unsigned).map_err(|error| error.to_string())?;
    let evidence_digest = format!("sha256:{}", hex::encode(Sha256::digest(&canonical)));
    let signature = hex::encode(signing.sign(&canonical).to_bytes());
    Ok(StagingEvidence {
        schema_version: 1,
        status: "passed".into(),
        release_digest,
        endpoint_identities,
        schema_revisions,
        request_ids,
        operation_ids,
        served_route: stream.served_route,
        usage_input_tokens: stream.usage.input_tokens,
        usage_output_tokens: stream.usage.output_tokens,
        redacted_trace_refs,
        evidence_digest,
        signing_key_id,
        signing_public_key,
        signature,
    })
}

pub fn verify_staging_report(evidence: &StagingEvidence) -> Result<(), String> {
    let unsigned = UnsignedEvidence {
        schema_version: evidence.schema_version,
        status: &evidence.status,
        release_digest: &evidence.release_digest,
        endpoint_identities: &evidence.endpoint_identities,
        schema_revisions: &evidence.schema_revisions,
        request_ids: &evidence.request_ids,
        operation_ids: &evidence.operation_ids,
        served_route: &evidence.served_route,
        usage_input_tokens: evidence.usage_input_tokens,
        usage_output_tokens: evidence.usage_output_tokens,
        redacted_trace_refs: &evidence.redacted_trace_refs,
        signing_key_id: &evidence.signing_key_id,
        signing_public_key: &evidence.signing_public_key,
    };
    let canonical = serde_json::to_vec(&unsigned).map_err(|error| error.to_string())?;
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&canonical)));
    if digest != evidence.evidence_digest {
        return Err("staging evidence digest mismatch".into());
    }
    let public: [u8; 32] = hex::decode(&evidence.signing_public_key)
        .map_err(|_| "invalid signing public key hex".to_string())?
        .try_into()
        .map_err(|_| "signing public key must be 32 bytes".to_string())?;
    let verifying = VerifyingKey::from_bytes(&public).map_err(|error| error.to_string())?;
    let expected_key_id = format!(
        "ed25519:{}",
        hex::encode(Sha256::digest(verifying.as_bytes()))
    );
    if expected_key_id != evidence.signing_key_id {
        return Err("staging signing key id mismatch".into());
    }
    let signature: [u8; 64] = hex::decode(&evidence.signature)
        .map_err(|_| "invalid signature hex".to_string())?
        .try_into()
        .map_err(|_| "signature must be 64 bytes".to_string())?;
    verifying
        .verify(&canonical, &Signature::from_bytes(&signature))
        .map_err(|error| error.to_string())
}

pub fn write_staging_report(path: &Path) -> Result<StagingEvidence, String> {
    let evidence = run_staging_readiness()?;
    let bytes = serde_json::to_vec_pretty(&evidence).map_err(|error| error.to_string())?;
    std::fs::write(path, bytes).map_err(|error| error.to_string())?;
    Ok(evidence)
}
