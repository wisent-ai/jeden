use crate::fleet::{run_db, sql};
use crate::model_router::{ChatConfig, CompletionUsage};
use rust_decimal::Decimal;
use std::{
    str::FromStr,
    sync::{Arc, Mutex},
};

// One machine request owns the process. Keep its accounting active until process exit,
// including model workers that finish after their original caller has returned.
static ACTIVE: Mutex<Option<Arc<Budget>>> = Mutex::new(None);
/// The request whose calls this process accounts, in the fleet database's
/// `pursuit_calls`; reservations of one request serialize on its ledger lock.
struct Budget {
    request: String,
    ledger: Mutex<()>,
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
pub(super) fn activate(request: &str, limit: &str) -> Result<(), String> {
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
    *active = Some(Arc::new(Budget {
        request: request.to_owned(),
        ledger: Mutex::new(()),
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
    let _ledger = budget
        .ledger
        .lock()
        .map_err(|_| "request budget ledger lock failed")?;
    let request = budget.request.clone();
    let limit = budget.limit;
    let id = uuid::Uuid::new_v4().to_string();
    let (row_id, model_name, revision) = (
        id.clone(),
        model.to_owned(),
        catalog.catalog_revision.clone(),
    );
    run_db(move |client| {
        let mut tx = client.transaction().map_err(sql)?;
        let mut allocated = Decimal::ZERO;
        for row in tx
            .query(
                "SELECT reserved,actual FROM pursuit_calls WHERE request=$1 FOR UPDATE",
                &[&request],
            )
            .map_err(sql)?
        {
            let reserved: String = row.get(0);
            let actual: Option<String> = row.get(1);
            allocated += Decimal::from_str(actual.as_deref().unwrap_or(&reserved))
                .map_err(|e| e.to_string())?;
        }
        if allocated + upper > limit {
            return Err(format!("budget_exhausted: limit {limit}, spent or still reserved {allocated}, next model attempt requires {upper}"));
        }
        tx.execute(
            "INSERT INTO pursuit_calls(request,id,model,catalog_revision,reserved,actual)
             VALUES ($1,$2,$3,$4,$5,NULL)",
            &[
                &request,
                &row_id,
                &model_name,
                &revision,
                &upper.to_string(),
            ],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    })?;
    drop(_ledger);
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
    let (request, id) = (reservation.budget.request.clone(), reservation.id.clone());
    let settled = actual.to_string();
    run_db(move |client| {
        client
            .execute(
                "UPDATE pursuit_calls SET actual=$1 WHERE request=$2 AND id=$3",
                &[&settled, &request, &id],
            )
            .map_err(sql)?;
        Ok(())
    })?;
    if actual > reservation.upper {
        return Err(
            "model usage exceeded its advertised bound; no further budget is granted".into(),
        );
    }
    Ok(())
}

pub(super) fn spent(request: &str) -> Result<Option<String>, String> {
    let request = request.to_owned();
    let actuals: Vec<Option<String>> = run_db(move |client| {
        Ok(client
            .query(
                "SELECT actual FROM pursuit_calls WHERE request=$1",
                &[&request],
            )
            .map_err(sql)?
            .into_iter()
            .map(|row| row.get(0))
            .collect())
    })?;
    let mut total = Decimal::ZERO;
    for actual in actuals {
        let Some(actual) = actual else {
            return Ok(None);
        };
        total += Decimal::from_str(&actual).map_err(|e| e.to_string())?;
    }
    Ok(Some(total.to_string()))
}
