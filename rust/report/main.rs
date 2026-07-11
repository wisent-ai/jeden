use ed25519_dalek::SigningKey;
use jeden::report::{
    canonical_envelope_json, markdown_from_machine_report, scan_private_data, verify_report,
    Aggregator, DsseEnvelope, TrustedRoot, EVIDENCE_PAYLOAD_TYPE,
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("quality-report: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("build") if args.len() == 10 => build(&args[1..]),
        Some("verify") if args.len() == 3 => verify_file(&args[1], &args[2]),
        Some("privacy-scan") if args.len() > 1 => privacy_scan(&args[1..]),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: jeden-quality-report build <evidence-dir> <evidence-root.json> <signing-seed-hex> <revision> <environment> <now-epoch-seconds> <max-age-seconds> <machine.json> <human.md>\n       jeden-quality-report verify <machine.json> <report-root.json>\n       jeden-quality-report privacy-scan <file>...".into()
}

fn build(args: &[String]) -> Result<(), String> {
    let evidence_dir = Path::new(&args[0]);
    let evidence_root: TrustedRoot = read_json(Path::new(&args[1]))?;
    let seed: [u8; 32] = hex::decode(&args[2])
        .map_err(|_| "signing seed must be hex".to_string())?
        .try_into()
        .map_err(|_| "signing seed must be exactly 32 bytes".to_string())?;
    let now = args[5]
        .parse::<u64>()
        .map_err(|_| "now must be u64".to_string())?;
    let max_age = args[6]
        .parse::<u64>()
        .map_err(|_| "max age must be u64".to_string())?;
    let mut paths = fs::read_dir(evidence_dir)
        .map_err(|error| format!("cannot read evidence directory: {error}"))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<PathBuf>, String>>()?;
    paths.retain(|path| path.extension().and_then(|value| value.to_str()) == Some("json"));
    paths.sort();
    let envelopes = paths
        .iter()
        .map(|path| read_json::<DsseEnvelope>(path))
        .collect::<Result<Vec<_>, _>>()?;
    if envelopes
        .iter()
        .any(|envelope| envelope.payload_type != EVIDENCE_PAYLOAD_TYPE)
    {
        return Err("evidence directory contains a non-evidence DSSE envelope".into());
    }
    let report_key = SigningKey::from_bytes(&seed);
    let envelope = Aggregator::new(&args[3], &args[4], now, max_age).aggregate(
        &envelopes,
        &evidence_root,
        &report_key,
    )?;
    let machine = canonical_envelope_json(&envelope)?;
    let report_root = TrustedRoot {
        ed25519_keys: std::collections::BTreeMap::from([(
            jeden::report::key_id(&report_key.verifying_key()),
            hex::encode(report_key.verifying_key().as_bytes()),
        )]),
    };
    let markdown = markdown_from_machine_report(&machine, &report_root)?;
    scan_private_data(&machine)?;
    scan_private_data(markdown.as_bytes())?;
    fs::write(&args[7], machine).map_err(|error| error.to_string())?;
    fs::write(&args[8], markdown).map_err(|error| error.to_string())?;
    Ok(())
}

fn verify_file(machine_path: &str, root_path: &str) -> Result<(), String> {
    let bytes = fs::read(machine_path).map_err(|error| error.to_string())?;
    let envelope: DsseEnvelope =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if canonical_envelope_json(&envelope)? != bytes {
        return Err("machine report envelope is not canonical JSON".into());
    }
    let root: TrustedRoot = read_json(Path::new(root_path))?;
    verify_report(&envelope, &root)?;
    Ok(())
}

fn privacy_scan(paths: &[String]) -> Result<(), String> {
    for path in paths {
        scan_private_data(&fs::read(path).map_err(|error| error.to_string())?)?;
    }
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?)
        .map_err(|error| format!("{}: {error}", path.display()))
}
