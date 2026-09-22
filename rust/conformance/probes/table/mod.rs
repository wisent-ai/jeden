//! The inventory of what each conformance area is checked against.

use super::AreaProbe;
use std::sync::LazyLock;

macro_rules! source {
    ($id:literal, $path:literal, [$($symbol:literal),+ $(,)?]) => {
        SourceProbe { id: $id, path: $path, symbols: &[$($symbol),+] }
    };
}

mod runtime;
mod surfaces;

pub(crate) static AREA_PROBES: LazyLock<Vec<&'static AreaProbe>> = LazyLock::new(|| {
    runtime::PROBES
        .iter()
        .chain(surfaces::PROBES.iter())
        .collect()
});
