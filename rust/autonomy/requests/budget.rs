use crate::fleet::{run_db, sql};
use crate::model_router::{ChatConfig, CompletionUsage};
use rust_decimal::Decimal;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait,
};
use std::{
    num::NonZeroU64,
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard},
};

// One machine request owns the process. Keep its accounting active until process exit,
// including model workers that finish after their original caller has returned.
static ACTIVE: Mutex<Option<Arc<Budget>>> = Mutex::new(None);

/// The tokens one catalogue price is the cost of, as the catalogue's own
/// model declares it.
const TOKENS_PER_PRICE: u32 = crate::control_plane::brama::ModelPrice::TOKENS_PER_PRICE;

/// The request whose calls this process accounts, in the fleet database's
/// `pursuit_calls`; reservations of one request serialize on its ledger lock.
struct Budget {
    request: String,
    ledger: Mutex<()>,
    limit: Decimal,
}

/// `pursuit_calls`: one reserved model attempt of a request and, once
/// settled, what it actually cost.
mod call {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "pursuit_calls")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub request: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub model: String,
        pub catalog_revision: String,
        pub reserved: String,
        pub actual: Option<String>,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

/// A route's stated price per million tokens, every side of it.
#[derive(Clone, Copy)]
struct Rates {
    input: Decimal,
    output: Decimal,
    cache_read: Decimal,
    cache_write: Decimal,
}

pub(crate) struct Reservation {
    budget: Arc<Budget>,
    id: String,
    rates: Rates,
    upper: Decimal,
}

fn decimal(value: f64) -> Result<Decimal, String> {
    let nothing: f64 = Default::default();
    if !value.is_finite() || value < nothing {
        return Err(format!(
            "model accounting received {value}, a negative or nonfinite amount"
        ));
    }
    Decimal::from_str(&value.to_string()).map_err(|e| e.to_string())
}

/// `lock`, waited for in the open: a lock another thread holds is said on
/// stderr while this one waits, and a poisoned one is refused naming it.
fn held<'a, T>(lock: &'a Mutex<T>, what: &str) -> Result<MutexGuard<'a, T>, String> {
    stado_wait::blocking(stado_wait::Kind::Lock, what, "this process", || lock.lock())
        .map_err(|poisoned| format!("{what} lock is poisoned: {poisoned}"))
}

pub(super) fn activate(request: &str, limit: &str) -> Result<(), String> {
    let mut active = held(&ACTIVE, "the request budget owner")?;
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

/// Every side of `price` the bound needs, or the refusal naming the side
/// the catalogue states no price for: an unstated side cannot be bounded.
fn stated_rates(
    model: &str,
    price: &crate::control_plane::brama::ModelPrice,
) -> Result<Rates, String> {
    let side = |name: &str, rate: Option<f64>| -> Result<Decimal, String> {
        match rate {
            Some(rate) => decimal(rate),
            None => Err(format!(
                "budget admission cannot bound {model}: the catalogue states no {name} price for it"
            )),
        }
    };
    Ok(Rates {
        input: side("input", price.input)?,
        output: side("output", price.output)?,
        cache_read: side("cacheRead", price.cache_read)?,
        cache_write: side("cacheWrite", price.cache_write)?,
    })
}

/// Admission is at each real HTTP attempt, including routing retries and compaction.
pub(crate) fn reserve(
    config: &ChatConfig,
    model: &str,
    max_tokens: Option<usize>,
) -> Result<Option<Reservation>, String> {
    let Some(budget) = held(&ACTIVE, "the request budget owner")?.clone() else {
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
    let Some(price) = entry.price.as_ref() else {
        return Err(format!("budget admission requires a priced concrete route with declared token ceilings; the catalogue states no price for {model}"));
    };
    let rates = stated_rates(model, price)?;
    let (Some(context_window), Some(ceiling)) = (
        entry.context_window.and_then(NonZeroU64::new),
        entry.max_output_tokens.and_then(NonZeroU64::new),
    ) else {
        return Err(format!("budget admission requires a priced concrete route with declared token ceilings; {model} declares no context window or output ceiling"));
    };
    if rates.input <= Decimal::ZERO || rates.output <= Decimal::ZERO {
        return Err(format!("budget admission requires a priced concrete route with declared token ceilings; {model} states a free input or output side"));
    }
    // The caller's own cap when it set one; the route's declared ceiling is
    // what an uncapped request may spend.
    let output = match max_tokens {
        Some(requested) => requested as u64,
        None => ceiling.get(),
    };
    if output > ceiling.get() {
        return Err(format!(
            "requested output exceeds {model}'s advertised token ceiling"
        ));
    }
    let input_rate = rates.input + rates.cache_read + rates.cache_write;
    let upper = (Decimal::from(context_window.get()) * input_rate
        + Decimal::from(output) * rates.output)
        / Decimal::from(TOKENS_PER_PRICE);
    let ledger = held(&budget.ledger, "the request budget ledger")?;
    let request = budget.request.clone();
    let limit = budget.limit;
    let id = uuid::Uuid::new_v4().to_string();
    let row = call::ActiveModel {
        request: Set(request.clone()),
        id: Set(id.clone()),
        model: Set(model.to_owned()),
        catalog_revision: Set(catalog.catalog_revision.clone()),
        reserved: Set(upper.to_string()),
        actual: Set(None),
    };
    run_db(move |db| async move {
        let tx = db.begin().await.map_err(sql)?;
        let place = format!("request {request}");
        let calls = stado_wait::until(
            stado_wait::Kind::Lock,
            "the request's pursuit_calls rows",
            place,
            call::Entity::find()
                .filter(call::Column::Request.eq(request))
                .lock_exclusive()
                .all(&tx),
        )
        .await
        .map_err(sql)?;
        let mut allocated = Decimal::ZERO;
        for call in calls {
            // A settled call counts what it cost; an unsettled one what it reserved.
            let counted = match call.actual.as_deref() {
                Some(actual) => actual,
                None => call.reserved.as_str(),
            };
            allocated += Decimal::from_str(counted).map_err(|e| {
                format!(
                    "pursuit call {} records {counted:?}, not an amount: {e}",
                    call.id
                )
            })?;
        }
        if allocated + upper > limit {
            return Err(format!("budget_exhausted: limit {limit}, spent or still reserved {allocated}, next model attempt requires {upper}"));
        }
        call::Entity::insert(row)
            .exec_without_returning(&tx)
            .await
            .map_err(sql)?;
        tx.commit().await.map_err(sql)
    })?;
    drop(ledger);
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
    let rates = reservation.rates;
    let actual = (decimal(usage.input_tokens)? * rates.input
        + decimal(usage.output_tokens)? * rates.output
        + decimal(usage.cache_read_tokens)? * rates.cache_read
        + decimal(usage.cache_write_tokens)? * rates.cache_write)
        / Decimal::from(TOKENS_PER_PRICE);
    let (request, id) = (reservation.budget.request.clone(), reservation.id.clone());
    let settled = actual.to_string();
    run_db(move |db| async move {
        call::Entity::update_many()
            .col_expr(
                call::Column::Actual,
                sea_orm::sea_query::Expr::value(settled),
            )
            .filter(call::Column::Request.eq(request))
            .filter(call::Column::Id.eq(id))
            .exec(&db)
            .await
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
    let actuals = run_db(move |db| async move {
        Ok(call::Entity::find()
            .filter(call::Column::Request.eq(request))
            .all(&db)
            .await
            .map_err(sql)?
            .into_iter()
            .map(|call| call.actual)
            .collect::<Vec<_>>())
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
