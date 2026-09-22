//! The signed-manifest steps of stable promotion: verify a canary manifest and
//! its artifact, re-issue the payload for the stable channel, wrap the KMS
//! signature in an envelope, and write the promotion record.
//!
//! Each action reads the workflow's environment, writes the files the next
//! step (`openssl pkeyutl -verify`, the release store upload) consumes, and
//! prints `key=value` step outputs.

use super::{digest_file, hex, now, utc_stamp, SECONDS_PER_DAY};
use crate::repository_root;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const USAGE: &str =
    "usage: jeden-tools release dsse <verify-canary | stable-payload | stable-envelope | promotion-record>";
/// Where the promotion record's layout, and so the gates it must name, is
/// declared.
const PROMOTION_SCHEMA: &str = "release/schema/promotion-record-v1.schema.json";
/// How long a stable manifest stays valid before it must be re-signed.
const STABLE_VALIDITY_DAYS: u64 = 30;
/// The layout the promotion record schema declares as `schemaVersion`.
const PROMOTION_RECORD_SCHEMA: u64 = 1;
/// The manifest layout a canary must carry to be promoted.
const MANIFEST_SCHEMA: u64 = 2;

pub(super) fn run(arguments: &[String]) -> Result<u8, String> {
    match arguments {
        [action] if action == "verify-canary" => verify_canary(),
        [action] if action == "stable-payload" => stable_payload(),
        [action] if action == "stable-envelope" => stable_envelope(),
        [action] if action == "promotion-record" => promotion_record(),
        _ => Err(USAGE.into()),
    }
    .map(|()| 0)
}

fn required(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|error| format!("{path}: {error}"))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// Sorted keys, no whitespace, non-ASCII escaped: the one byte form a DSSE
/// payload is signed in, so re-encoding a decoded payload reproduces it.
pub(crate) fn canonical(value: &Value) -> String {
    let compact = value.to_string();
    let mut escaped = String::with_capacity(compact.len());
    for character in compact.chars() {
        if character.is_ascii() {
            escaped.push(character);
        } else {
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units) {
                escaped.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    escaped
}

/// DSSE pre-authentication encoding: what the signature actually covers.
fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let mut encoded = format!(
        "DSSEv1 {} {payload_type} {} ",
        payload_type.len(),
        payload.len()
    )
    .into_bytes();
    encoded.extend_from_slice(payload);
    encoded
}

fn decoded(value: &Value, what: &str) -> Result<Vec<u8>, String> {
    let text = value.as_str().ok_or_else(|| format!("{what} is missing"))?;
    STANDARD
        .decode(text)
        .map_err(|error| format!("{what} is not valid base64: {error}"))
}

fn entries(directory: &Path) -> Result<Vec<PathBuf>, String> {
    fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", directory.display()))
}

/// The gates every promotion must name, as the promotion record schema
/// declares them.
fn declared_gates() -> Result<Vec<String>, String> {
    let schema = read_json(&repository_root().join(PROMOTION_SCHEMA))?;
    let gates = schema["properties"]["gateDigests"]["required"]
        .as_array()
        .ok_or_else(|| format!("{PROMOTION_SCHEMA} declares no required gate digests"))?
        .iter()
        .map(|gate| gate.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| format!("{PROMOTION_SCHEMA} names a gate that is not a string"))?;
    if gates.is_empty() {
        return Err(format!(
            "{PROMOTION_SCHEMA} declares no required gate digests"
        ));
    }
    Ok(gates)
}

fn verify_canary() -> Result<(), String> {
    let roots = entries(Path::new("canary"))?
        .into_iter()
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    let [root] = roots.as_slice() else {
        return Err(format!(
            "expected exactly one canary artifact, got {}",
            roots.len()
        ));
    };
    let envelope_path = root.join("manifest.dsse.json");
    let envelope_bytes = fs::read(&envelope_path)
        .map_err(|error| format!("{}: {error}", envelope_path.display()))?;
    let envelope: Value = serde_json::from_slice(&envelope_bytes)
        .map_err(|error| format!("{}: {error}", envelope_path.display()))?;
    let payload_type = required("DSSE_PAYLOAD_TYPE")?;
    if envelope["payloadType"] != payload_type.as_str() {
        return Err("wrong DSSE payload type".into());
    }
    let payload_bytes = decoded(&envelope["payload"], "canary payload")?;
    let payload: Value = serde_json::from_slice(&payload_bytes)
        .map_err(|error| format!("canary payload: {error}"))?;
    if canonical(&payload).as_bytes() != payload_bytes {
        return Err("non-canonical canary payload".into());
    }
    if payload["schemaVersion"] != MANIFEST_SCHEMA || payload["channel"] != "canary" {
        return Err("input is not ReleaseManifestV2 canary".into());
    }
    if payload["version"] != required("EXPECTED_VERSION")?.as_str()
        || payload["targetTriple"] != required("EXPECTED_TARGET")?.as_str()
    {
        return Err("version/target mismatch".into());
    }
    let archives = entries(root)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("jeden-") && name.ends_with(".tar.gz"))
        })
        .collect::<Vec<_>>();
    let [artifact] = archives.as_slice() else {
        return Err("expected exactly one immutable artifact archive".into());
    };
    let digest = digest_file(artifact)?;
    let size = artifact
        .metadata()
        .map_err(|error| error.to_string())?
        .len();
    if payload["sha256"] != digest.as_str() || payload["size"] != size {
        return Err("canary artifact digest/size mismatch".into());
    }
    let gates = read_json(&root.join("release-gate-digests.json"))?;
    let commit = Regex::new("^[0-9a-f]{40}$").map_err(|error| error.to_string())?;
    let source_sha = gates["sourceSha"]
        .as_str()
        .filter(|value| commit.is_match(value));
    let Some(source_sha) = source_sha.filter(|_| gates["artifactSha256"] == digest.as_str()) else {
        return Err("invalid release gate identity".into());
    };
    let required_gates: Map<String, Value> = serde_json::from_str(&required("REQUIRED_GATES")?)
        .map_err(|error| format!("REQUIRED_GATES: {error}"))?;
    let declared = declared_gates()?;
    let sha256 = Regex::new("^[0-9a-f]{64}$").map_err(|error| error.to_string())?;
    if required_gates.len() != declared.len()
        || declared.iter().any(|gate| {
            !required_gates
                .get(gate)
                .and_then(Value::as_str)
                .is_some_and(|value| sha256.is_match(value))
        })
    {
        return Err(format!(
            "exactly the gate digests {PROMOTION_SCHEMA} requires are needed: {}",
            declared.join(", ")
        ));
    }
    let signature = decoded(&envelope["signatures"][0]["sig"], "canary signature")?;
    write("canary.pae", &pae(&payload_type, &payload_bytes))?;
    write("canary.sig", &signature)?;
    write(
        "canary-public.pem",
        required("CANARY_PUBLIC_KEY_PEM")?.as_bytes(),
    )?;
    println!("root={}", root.display());
    println!("artifact_sha256={digest}");
    println!(
        "canary_manifest_sha256={}",
        hex(&Sha256::digest(&envelope_bytes))
    );
    println!("source_sha={source_sha}");
    println!("gate_digests={}", canonical(&Value::Object(required_gates)));
    Ok(())
}

fn stable_payload() -> Result<(), String> {
    let root = PathBuf::from(required("CANARY_ROOT")?);
    let envelope = read_json(&root.join("manifest.dsse.json"))?;
    let mut payload: Value =
        serde_json::from_slice(&decoded(&envelope["payload"], "canary payload")?)
            .map_err(|error| format!("canary payload: {error}"))?;
    let published = now()?;
    let object = payload
        .as_object_mut()
        .ok_or("canary payload is not an object")?;
    object.insert("channel".into(), json!("stable"));
    object.insert("keyId".into(), json!(required("STABLE_KMS_KEY_ID")?));
    object.insert("publishedAt".into(), json!(utc_stamp(published)));
    object.insert(
        "expiresAt".into(),
        json!(utc_stamp(
            published + STABLE_VALIDITY_DAYS * SECONDS_PER_DAY
        )),
    );
    if payload["sha256"] != required("ARTIFACT_SHA256")?.as_str() {
        return Err("promotion attempted to change artifact digest".into());
    }
    let raw = canonical(&payload).into_bytes();
    let signed = pae(&required("DSSE_PAYLOAD_TYPE")?, &raw);
    write("stable.payload", &raw)?;
    write("stable.pae", &signed)?;
    println!("payload_base64={}", STANDARD.encode(&raw));
    println!("pae_base64={}", STANDARD.encode(&signed));
    Ok(())
}

fn stable_envelope() -> Result<(), String> {
    let signature = required("SIGNATURE")?;
    write(
        "stable.sig",
        &decoded(&json!(signature), "stable signature")?,
    )?;
    let envelope = json!({
        "payloadType": required("DSSE_PAYLOAD_TYPE")?,
        "payload": required("PAYLOAD")?,
        "signatures": [{"keyid": required("STABLE_KMS_KEY_ID")?, "sig": signature}],
    });
    write("stable.dsse.json", (canonical(&envelope) + "\n").as_bytes())?;
    write(
        "stable-public.pem",
        required("STABLE_PUBLIC_KEY_PEM")?.as_bytes(),
    )
}

fn promotion_record() -> Result<(), String> {
    let stable_digest = digest_file(Path::new("stable.dsse.json"))?;
    let artifact = required("ARTIFACT_SHA256")?;
    let gate_digests: Value = serde_json::from_str(&required("GATE_DIGESTS")?)
        .map_err(|error| format!("GATE_DIGESTS: {error}"))?;
    let record = json!({
        "schemaVersion": PROMOTION_RECORD_SCHEMA,
        "sourceSha": required("SOURCE_SHA")?,
        "version": required("PROMOTED_VERSION")?,
        "targetTriple": required("PROMOTED_TARGET")?,
        "canaryArtifactSha256": artifact,
        "stableArtifactSha256": artifact,
        "canaryManifestSha256": required("CANARY_MANIFEST_SHA256")?,
        "stableManifestSha256": stable_digest,
        "gateDigests": gate_digests,
        "approvals": [
            "github-environment:release-stable",
            format!("actor:{}", required("GITHUB_ACTOR")?),
        ],
        "promotedAt": utc_stamp(now()?),
        "workflowRun": format!("{}/{}", required("GITHUB_RUN_ID")?, required("GITHUB_RUN_ATTEMPT")?),
    });
    write(
        "promotion-record.json",
        (canonical(&record) + "\n").as_bytes(),
    )?;
    write("stable.digest", format!("{stable_digest}\n").as_bytes())
}
