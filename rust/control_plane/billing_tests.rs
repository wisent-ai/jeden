use super::billing::{
    self, BillingErrorCode, BillingEvent, BillingGrant, OperationResult, OperationState,
    PaymentMethodReference, PolicyPeriod, PurchasePolicy, PurchaseRequest,
};
use serde_json::{json, Value};
use std::fs;

const PAYMENT_SENTINEL: &str = "JEDEN_TEST_PAYMENT_SECRET_4111111111111111";

fn payment_reference(value: &str) -> PaymentMethodReference {
    serde_json::from_value(Value::String(value.into())).expect("valid opaque payment reference")
}

fn enabled_policy() -> PurchasePolicy {
    PurchasePolicy {
        enabled: true,
        auto_renew: true,
        allowed_products: vec!["provider/pro".into()],
        allowed_currencies: vec!["USD".into()],
        max_single_microunits: 5_000_000,
        max_period_microunits: 20_000_000,
        period: PolicyPeriod::BillingCycle,
        revision: "policy-rev-7".into(),
        valid_until_ms: 4_102_444_800_000,
    }
}

#[test]
fn billing_v2_fixture_declares_the_complete_security_and_denial_matrix() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/control_plane/contract-v2.json");
    let bytes = fs::read(&path).expect("billing v2 fixture exists");
    let fixture: Value = serde_json::from_slice(&bytes).expect("billing v2 fixture is JSON");
    assert_eq!(fixture["schemaVersion"], 2);
    assert_eq!(fixture["fixtureId"], "control-plane-billing-contract-v2");
    assert_eq!(fixture["transport"], "injected");
    assert_eq!(fixture["service"], "weles");
    assert_eq!(fixture["apiPrefix"], "/v2");
    assert_eq!(fixture["security"]["paymentAuthority"], "weles");
    assert_eq!(fixture["security"]["hostedSetupOnly"], true);
    assert_eq!(fixture["security"]["rawPaymentDetailsAccepted"], false);
    assert_eq!(fixture["security"]["responseBodiesSuppressedOnError"], true);
    assert_eq!(
        fixture["security"]["financialMutationIdempotency"],
        "caller-stable-required"
    );

    let cases = fixture["cases"].as_array().expect("cases array");
    assert!(
        cases.len() >= 50,
        "billing fixture silently lost contract branches"
    );
    for required in [
        "payment-method-setup-hosted-https-allowlist",
        "payment-method-setup-rejects-http",
        "purchase-policy-default-disabled",
        "purchase-policy-stale-revision",
        "purchase-policy-single-cap-boundary",
        "purchase-policy-single-cap-exceeded",
        "purchase-policy-period-cap-boundary",
        "purchase-policy-period-cap-exceeded",
        "purchase-policy-arithmetic-overflow",
        "quote-expired",
        "purchase-replay-same-key-same-body",
        "purchase-conflict-same-key-different-body",
        "renew-concurrent-single-charge",
        "cancel-complete-race-idempotent",
        "typed-401",
        "typed-403",
        "typed-404",
        "typed-409",
        "typed-422",
        "typed-428",
        "typed-429-retry-after",
        "typed-503",
        "oversize-response",
        "encoded-account-segment",
        "encoded-subscription-segment",
        "encoded-payment-reference-segment",
        "secret-absent-from-debug-error-ledger-fixture",
    ] {
        assert!(
            cases.iter().any(|case| case == required),
            "missing billing fixture case {required}"
        );
    }
    assert!(!String::from_utf8_lossy(&bytes).contains(PAYMENT_SENTINEL));
}

#[test]
fn billing_staging_schema_and_prerequisites_remain_strict_and_weles_bound() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let schema: Value = serde_json::from_slice(
        &fs::read(root.join("tests/staging/control-plane-e2e.schema.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"]["schemaVersion"]["enum"]
        .as_array()
        .is_some_and(|versions| versions.iter().any(|version| version == 2)));
    let required = schema["required"].as_array().unwrap();
    for name in [
        "endpointIdentities",
        "schemaRevisions",
        "requestIds",
        "evidenceDigest",
    ] {
        assert!(required.iter().any(|entry| entry == name));
    }
    let prerequisites: Value = serde_json::from_slice(
        &fs::read(root.join("tests/staging/control-plane-prerequisites.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(prerequisites["missingStatus"], "external-blocked");
    let environments = prerequisites["prerequisites"].as_array().unwrap();
    assert!(environments
        .iter()
        .any(|entry| entry["environment"] == "WELES_STAGING_URL"));
    assert!(environments
        .iter()
        .any(|entry| entry["environment"] == "JEDEN_STAGING_TENANT"));
    assert!(!serde_json::to_string(&prerequisites)
        .unwrap()
        .contains(PAYMENT_SENTINEL));
}

#[test]
fn payment_credentials_are_not_representable_by_billing_request_dtos() {
    let reference = payment_reference("pmref_opaque_7");
    let valid = json!({
        "quoteId": "quote-1",
        "quoteRevision": "quote-rev-1",
        "policyRevision": "policy-rev-7",
        "paymentMethodReference": reference,
    });
    let parsed: PurchaseRequest = serde_json::from_value(valid.clone()).unwrap();
    assert_eq!(parsed.payment_method_reference.as_str(), "pmref_opaque_7");

    for forbidden in [
        "pan",
        "cardNumber",
        "cvv",
        "cvc",
        "processorToken",
        "fullAddress",
    ] {
        let mut adversarial = valid.clone();
        adversarial
            .as_object_mut()
            .unwrap()
            .insert(forbidden.into(), Value::String(PAYMENT_SENTINEL.into()));
        let error = serde_json::from_value::<PurchaseRequest>(adversarial)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unknown field"),
            "{forbidden} was not rejected: {error}"
        );
        assert!(
            !error.contains(PAYMENT_SENTINEL),
            "serde error leaked payment material"
        );
    }
}

#[test]
fn opaque_payment_material_is_redacted_from_debug_and_typed_operation_errors() {
    let reference = payment_reference(PAYMENT_SENTINEL);
    assert_eq!(
        format!("{reference:?}"),
        "PaymentMethodReference([REDACTED])"
    );
    let grant: BillingGrant =
        serde_json::from_value(Value::String(PAYMENT_SENTINEL.into())).unwrap();
    assert_eq!(format!("{grant:?}"), "BillingGrant([REDACTED])");

    let result = OperationResult {
        operation_id: "operation-7".into(),
        state: OperationState::Rejected,
        events: vec![],
        error: Some(super::billing::BillingOperationError {
            code: BillingErrorCode::PaymentMethodUnavailable,
            retry_after_ms: None,
            current_policy_revision: Some("policy-rev-8".into()),
        }),
    };
    for rendered in [
        format!("{result:?}"),
        serde_json::to_string(&result).unwrap(),
    ] {
        assert!(!rendered.contains(PAYMENT_SENTINEL));
    }

    let event = BillingEvent::PaymentMethodRevoked { reference };
    assert!(!format!("{event:?}").contains(PAYMENT_SENTINEL));
    assert!(
        serde_json::to_string(&event)
            .unwrap()
            .contains(PAYMENT_SENTINEL),
        "opaque references must remain wire-serializable to Weles"
    );
}

#[test]
fn opaque_references_and_response_collections_are_bounded() {
    assert!(
        serde_json::from_value::<PaymentMethodReference>(Value::String(String::new())).is_err()
    );
    assert!(
        serde_json::from_value::<PaymentMethodReference>(Value::String(
            "x".repeat(billing::MAX_BILLING_STRING_BYTES + 1)
        ))
        .is_err()
    );
    assert!(
        serde_json::from_value::<PaymentMethodReference>(Value::String("opaque\nreference".into()))
            .is_err()
    );
    assert!(serde_json::from_value::<BillingGrant>(Value::String(String::new())).is_err());
}

#[test]
fn purchase_policy_validation_covers_opt_in_caps_and_revision_denials() {
    let valid = enabled_policy();
    assert_eq!(
        valid.max_single_microunits, 5_000_000,
        "exact cap remains permitted"
    );
    billing::validate_policy(&valid).unwrap();

    let mut policy = valid.clone();
    policy.enabled = false;
    assert_eq!(
        billing::validate_policy(&policy),
        Err("auto-renew requires an enabled purchase policy")
    );

    let mut policy = valid.clone();
    policy.allowed_products.clear();
    assert_eq!(
        billing::validate_policy(&policy),
        Err("allowed product count is invalid")
    );

    let mut policy = valid.clone();
    policy.allowed_currencies.clear();
    assert_eq!(
        billing::validate_policy(&policy),
        Err("allowed currency count is invalid")
    );

    let mut policy = valid.clone();
    policy.max_single_microunits = policy.max_period_microunits + 1;
    assert_eq!(
        billing::validate_policy(&policy),
        Err("single purchase limit exceeds period limit")
    );

    let mut policy = valid;
    policy.revision.clear();
    assert_eq!(
        billing::validate_policy(&policy),
        Err("policy revision is invalid")
    );
}
