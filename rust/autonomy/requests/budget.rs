use crate::model_router::{ChatConfig, CompletionUsage};
use rusqlite::{params, Connection};
use rust_decimal::Decimal;
use std::{
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
};

// One machine request owns the process. Keep its accounting active until process exit,
// including model workers that finish after their original caller has returned.
static ACTIVE: Mutex<Option<Arc<Budget>>> = Mutex::new(None);
struct Budget {
    connection: Mutex<Connection>,
    limit: Decimal,
}
pub(crate) struct Reservation {
    budget: Arc<Budget>,
    id: String,
    rates: [Decimal; 4],
    upper: Decimal,
}

fn decimal(value: f64) -> Result<Decimal, String> {
    if !value.is_finite() || value < 0.0 {
        return Err("model accounting received a negative or nonfinite amount".into());
    }
    Decimal::from_str(&value.to_string()).map_err(|e| e.to_string())
}
pub(super) fn activate(directory: &Path, limit: &str) -> Result<(), String> {
    let mut active = ACTIVE
        .lock()
        .map_err(|_| "request budget owner lock failed")?;
    if active.is_some() {
        return Err("this process already owns a durable request budget".into());
    }
    let limit = Decimal::from_str(limit).map_err(|e| format!("invalid request budget: {e}"))?;
    if limit <= Decimal::ZERO {
        return Err("request budget must be positive".into());
    }
    let connection =
        Connection::open(directory.join("inference.sqlite3")).map_err(|e| e.to_string())?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS calls (id TEXT PRIMARY KEY, model TEXT NOT NULL, catalog_revision TEXT NOT NULL, reserved TEXT NOT NULL, actual TEXT);").map_err(|e| e.to_string())?;
    *active = Some(Arc::new(Budget {
        connection: Mutex::new(connection),
        limit,
    }));
    Ok(())
}

/// Admission is at each real HTTP attempt, including routing retries and compaction.
pub(crate) fn reserve(
    config: &ChatConfig,
    model: &str,
    max_tokens: Option<usize>,
) -> Result<Option<Reservation>, String> {
    let Some(budget) = ACTIVE
        .lock()
        .map_err(|_| "request budget owner lock failed")?
        .clone()
    else {
        return Ok(None);
    };
    let client = crate::control_plane::brama::BramaClient::configured(
        Some(config.url.clone()),
        Some(config.bearer_token.clone()),
    );
    let catalog = client
        .catalog(true)
        .map_err(|e| format!("budget pricing read failed: {e}"))?;
    let entry = catalog
        .resolve(model)
        .map_err(|e| format!("budget admission cannot price route {model}: {e}"))?;
    let rates = [
        decimal(entry.price.input)?,
        decimal(entry.price.output)?,
        decimal(entry.price.cache_read)?,
        decimal(entry.price.cache_write)?,
    ];
    if entry.context_window == 0
        || entry.max_output_tokens == 0
        || rates[0] <= Decimal::ZERO
        || rates[1] <= Decimal::ZERO
    {
        return Err(format!("budget admission requires a priced concrete route with declared token ceilings; {model} has incomplete pricing or capacity"));
    }
    let output = max_tokens
        .map(|v| v as u64)
        .unwrap_or(entry.max_output_tokens);
    if output > entry.max_output_tokens {
        return Err(format!(
            "requested output exceeds {model}'s advertised token ceiling"
        ));
    }
    let input_rate = rates[0] + rates[2] + rates[3];
    let million = Decimal::from(1_000_000u64);
    let upper = (Decimal::from(entry.context_window) * input_rate
        + Decimal::from(output) * rates[1])
        / million;
    let connection = budget
        .connection
        .lock()
        .map_err(|_| "request budget ledger lock failed")?;
    let tx = connection
        .unchecked_transaction()
        .map_err(|e| e.to_string())?;
    let mut statement = tx
        .prepare("SELECT reserved,actual FROM calls")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut allocated = Decimal::ZERO;
    for row in rows {
        let (reserved, actual) = row.map_err(|e| e.to_string())?;
        allocated +=
            Decimal::from_str(actual.as_deref().unwrap_or(&reserved)).map_err(|e| e.to_string())?;
    }
    drop(statement);
    if allocated + upper > budget.limit {
        return Err(format!("budget_exhausted: limit {}, spent or still reserved {allocated}, next model attempt requires {upper}",budget.limit));
    }
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO calls VALUES (?1,?2,?3,?4,NULL)",
        params![id, model, catalog.catalog_revision, upper.to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(connection);
    Ok(Some(Reservation {
        budget,
        id,
        rates,
        upper,
    }))
}

pub(crate) fn settle(
    reservation: Option<Reservation>,
    usage: Option<&CompletionUsage>,
) -> Result<(), String> {
    let Some(reservation) = reservation else {
        return Ok(());
    };
    let usage = usage.ok_or("model response omitted usage; its maximum cost remains reserved")?;
    let tokens = [
        decimal(usage.input_tokens)?,
        decimal(usage.output_tokens)?,
        decimal(usage.cache_read_tokens)?,
        decimal(usage.cache_write_tokens)?,
    ];
    let actual = tokens
        .iter()
        .zip(reservation.rates)
        .map(|(count, rate)| *count * rate)
        .sum::<Decimal>()
        / Decimal::from(1_000_000u64);
    reservation
        .budget
        .connection
        .lock()
        .map_err(|_| "request budget ledger lock failed")?
        .execute(
            "UPDATE calls SET actual=?1 WHERE id=?2",
            params![actual.to_string(), reservation.id],
        )
        .map_err(|e| e.to_string())?;
    if actual > reservation.upper {
        return Err(
            "model usage exceeded its advertised bound; no further budget is granted".into(),
        );
    }
    Ok(())
}

pub(super) fn spent(directory: &Path) -> Result<Option<String>, String> {
    let path = directory.join("inference.sqlite3");
    if !path.exists() {
        return Ok(Some(Decimal::ZERO.to_string()));
    }
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let mut statement = connection
        .prepare("SELECT actual FROM calls")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |r| r.get::<_, Option<String>>(0))
        .map_err(|e| e.to_string())?;
    let mut total = Decimal::ZERO;
    for row in rows {
        let Some(actual) = row.map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        total += Decimal::from_str(&actual).map_err(|e| e.to_string())?;
    }
    Ok(Some(total.to_string()))
}
