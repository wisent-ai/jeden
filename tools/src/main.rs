//! Jeden's repository tooling, in the one language the product is written in.
//!
//! - `release`: run Cargo against the declared private-source input, export
//!   that input, stage native binaries, and the release workflows' artifact
//!   facts and signed-manifest steps.
//! - `surface`: print the public command vocabulary the binary answers to.
//! - `versioning`: the fleet's versioning rule and the published baseline the
//!   version gate compares against.
//! - `protocol-check`: hold the jeden.session.v1 schema, its golden envelopes
//!   and the SDK sources to the protocol contract.
//!
//! Every command refuses with a named reason on standard error and a non-zero
//! exit status; none of them degrades to a smaller answer.

mod protocol;
mod release;
mod surface;
mod versioning;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: jeden-tools <release|surface|versioning|protocol-check> ...";

/// The repository this tool belongs to. The tool is built from `tools/` in
/// the checkout it serves, so its manifest directory's parent is that
/// checkout, on a developer host and on a release worker alike.
pub(crate) fn repository_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().map(PathBuf::from).unwrap_or(manifest)
}

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let Some((command, rest)) = arguments.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let outcome = match command.as_str() {
        "release" => release::run(rest),
        "surface" => surface::run(rest),
        "versioning" => versioning::run(rest),
        "protocol-check" => protocol::run(rest),
        other => Err(format!("unknown command `{other}`; {USAGE}")),
    };
    match outcome {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
