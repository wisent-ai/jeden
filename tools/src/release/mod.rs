//! The release side: Cargo against the declared private-source input, the
//! export that produces that input, native staging, and the facts and signed
//! manifests the release workflows record.

mod cargo;
mod dsse;
mod export;
mod stage;

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: jeden-tools release <export ARCHIVE | cargo ARGS... | stage --bin NAME... [--qualify TEST...] | facts FILE | dsse ...>";
/// Every refusal from the private-source commands carries this prefix, so a
/// release log names which layer refused.
const PREFIX: &str = "private Cargo sources: ";

pub(crate) fn run(arguments: &[String]) -> Result<u8, String> {
    let Some((action, rest)) = arguments.split_first() else {
        return Err(USAGE.into());
    };
    let outcome = match action.as_str() {
        "export" => match rest {
            [archive] => export::export(Path::new(archive)).map(|()| 0),
            _ => Err("export requires exactly one archive path under .wisent-output".into()),
        },
        "cargo" if rest.is_empty() => Err("cargo requires a Cargo command".into()),
        "cargo" => cargo::cargo(rest),
        "stage" => {
            let (binaries, qualifications) = stage_arguments(rest)?;
            stage::stage(&binaries, &qualifications)
        }
        "facts" => return facts(rest),
        "dsse" => return dsse::run(rest),
        other => return Err(format!("unknown release action `{other}`; {USAGE}")),
    };
    outcome.map_err(|message| format!("{PREFIX}{message}"))
}

/// `--bin NAME` pairs name what is staged; `--qualify TEST` pairs name the
/// ignored integration tests run against the staged candidate afterwards.
fn stage_arguments(arguments: &[String]) -> Result<(Vec<String>, Vec<String>), String> {
    let mut names = Vec::new();
    let mut qualifications = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(flag) = remaining.next() {
        match (flag.as_str(), remaining.next()) {
            ("--bin", Some(name)) if !name.is_empty() => names.push(name.clone()),
            ("--qualify", Some(test)) if !test.is_empty() => qualifications.push(test.clone()),
            _ => {
                return Err(format!(
                    "{PREFIX}stage takes only --bin NAME and --qualify TEST pairs"
                ))
            }
        }
    }
    if names.is_empty() {
        return Err(format!("{PREFIX}stage requires at least one --bin NAME"));
    }
    Ok((names, qualifications))
}

/// `name=`, `sha256=` and `size=` lines for one file, in the form a GitHub
/// step output takes, so a workflow names an artifact the same way on every
/// runner operating system.
fn facts(arguments: &[String]) -> Result<u8, String> {
    let [path] = arguments else {
        return Err("facts requires exactly one file".into());
    };
    let path = Path::new(path);
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file", path.display()))?;
    let size = path
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?
        .len();
    println!("name={}", name.to_string_lossy());
    println!("sha256={}", digest_file(path)?);
    println!("size={size}");
    Ok(0)
}

pub(crate) fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Run a command in the repository and return its standard output, refusing
/// with the command line when it fails.
pub(crate) fn output(command: &mut Command) -> Result<String, String> {
    let described = format!("{command:?}");
    let finished = command
        .current_dir(crate::repository_root())
        .output()
        .map_err(|error| format!("cannot start {described}: {error}"))?;
    if !finished.status.success() {
        return Err(format!(
            "command failed: {described}: {}",
            String::from_utf8_lossy(&finished.stderr).trim()
        ));
    }
    String::from_utf8(finished.stdout).map_err(|error| format!("{described}: {error}"))
}

pub(crate) const SECONDS_PER_DAY: u64 = 86_400;

/// `YYYY-MM-DDTHH:MM:SSZ` for a Unix time, without a calendar dependency.
pub(crate) fn utc_stamp(seconds: u64) -> String {
    let days = (seconds / SECONDS_PER_DAY) as i64;
    let time = seconds % SECONDS_PER_DAY;
    // Civil-from-days (H. Hinnant): days since 1970-01-01 to year, month, day.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

pub(crate) fn now() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs())
}
