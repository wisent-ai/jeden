//! Signing a rebuilt Jeden on macOS with the identity the running build
//! carries, so the operating system's grants to Jeden survive the update.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use super::checks::command_failure;

pub(super) enum Signer {
    /// The codesign identity the machine names in `JEDEN_CODESIGN_IDENTITY`
    /// (`-` for an ad-hoc signature); Stado is neither asked nor used.
    Own(String),
    /// Stado signs. With the running build's stable signature, the new build
    /// keeps its identifier and authority and must meet its requirement.
    Stado(Option<Value>),
}

impl Signer {
    pub(super) fn for_running_build() -> Result<Self, String> {
        match std::env::var("JEDEN_CODESIGN_IDENTITY") {
            Ok(identity) if !identity.trim().is_empty() => return Ok(Self::Own(identity)),
            Ok(_) | Err(std::env::VarError::NotPresent) => {}
            Err(error) => return Err(format!("JEDEN_CODESIGN_IDENTITY cannot be read: {error}")),
        }
        let previous = std::env::current_exe()
            .map_err(|error| format!("cannot identify the running Jeden executable: {error}"))?;
        let mut inspect = Command::new("stado");
        inspect
            .args(["product", "signing", "inspect", "--json"])
            .arg(previous);
        let report = stado_wait::output(&mut inspect).map_err(|error| {
            format!(
                "cannot inspect the running code identity through Stado: {error}; \
                 without Stado set JEDEN_CODESIGN_IDENTITY to a codesign identity, or - for ad-hoc"
            )
        })?;
        // A refused inspection is said with Stado's own sentence: parsed as
        // a report, it read as invalid JSON and the cause was lost.
        if !report.status.success() {
            return Err(command_failure(
                "stado product signing inspect",
                &report.stderr,
            ));
        }
        let reports: Vec<Value> = serde_json::from_slice(&report.stdout)
            .map_err(|error| format!("cannot read the running code identity: {error}"))?;
        let observed = reports
            .into_iter()
            .next()
            .ok_or("signing report was empty")?;
        match observed["state"].as_str() {
            Some("stable") => Ok(Self::Stado(Some(observed))),
            Some("adhoc" | "unsigned") => Ok(Self::Stado(None)),
            _ => Err(format!(
                "running code identity could not be verified: {observed}"
            )),
        }
    }

    pub(super) fn sign(&self, executable: &Path) -> Result<(), String> {
        match self {
            Self::Own(identity) => {
                let mut codesign = Command::new("/usr/bin/codesign");
                codesign
                    .args(["--force", "--sign"])
                    .arg(identity)
                    .arg(executable);
                let output = stado_wait::output(&mut codesign)
                    .map_err(|error| format!("cannot start codesign: {error}"))?;
                if !output.status.success() {
                    return Err(command_failure(
                        "codesign with JEDEN_CODESIGN_IDENTITY",
                        &output.stderr,
                    ));
                }
                Ok(())
            }
            Self::Stado(previous) => {
                let mut signer = Command::new("stado");
                signer.args(["product", "signing", "sign"]);
                match previous {
                    Some(previous) => signer.args([
                        "--identifier",
                        previous["identifier"]
                            .as_str()
                            .ok_or("signing report omitted identifier")?,
                        "--identity",
                        previous["authority"]
                            .as_str()
                            .ok_or("signing report omitted authority")?,
                    ]),
                    None => signer.args(["--product", "jeden"]),
                };
                let output = stado_wait::output(signer.arg(executable))
                    .map_err(|error| format!("cannot start stable macOS signing: {error}"))?;
                if !output.status.success() {
                    return Err(command_failure("stable macOS signing", &output.stderr));
                }
                match previous {
                    Some(previous) => keeps_identity(previous, executable),
                    None => Ok(()),
                }
            }
        }
    }
}

/// The new build meets the code requirement the running build carried.
fn keeps_identity(previous: &Value, executable: &Path) -> Result<(), String> {
    let requirement = previous["requirement"]
        .as_str()
        .ok_or("signing report omitted requirement")?;
    let mut verify = Command::new("/usr/bin/codesign");
    verify
        .args(["--verify", "--strict", "-R"])
        .arg(format!("={requirement}"))
        .arg(executable);
    let proof = stado_wait::output(&mut verify)
        .map_err(|error| format!("cannot verify update identity: {error}"))?;
    if !proof.status.success() {
        return Err(command_failure(
            "update changed macOS code identity",
            &proof.stderr,
        ));
    }
    Ok(())
}
