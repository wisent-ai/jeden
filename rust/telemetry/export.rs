use super::schema::TelemetryEnvelope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportError {
    Unavailable,
    Timeout,
    Rejected,
    InvalidConfiguration,
}

/// Transport-neutral private OTLP boundary. Implementations receive only the typed allowlisted
/// schema and are installed only after explicit user opt-in.
pub trait OtlpExporter: Send + Sync {
    fn export(&self, batch: &[TelemetryEnvelope]) -> Result<(), ExportError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportStatus {
    Disabled,
    Empty,
    Exported { records: usize },
    Failed(ExportError),
}
