use super::billing::QuotaBucket;
use super::contract::{RequestMeta, WelesApiV2};
use super::weles::WelesClient;

/// One resolved quota bucket with its display label fully composed
/// (account, product, and bucket qualifiers applied).
pub struct LabeledBucket {
    pub label: String,
    pub bucket: QuotaBucket,
}

/// One row of an account's quota listing: a resolved bucket, or the error
/// that prevented resolving one account/subscription level entry.
pub enum QuotaEntry {
    Bucket(LabeledBucket),
    Unavailable { label: String, error: String },
}

/// Quota entries of one Weles account.
pub struct AccountQuotas {
    pub provider: String,
    pub entries: Vec<QuotaEntry>,
}

/// Result of a live subscription quota fetch. Failures at the health or
/// account-list level collapse the whole fetch into `Unavailable`; failures
/// below that stay attached to their account so partial data still renders.
pub enum SubscriptionQuotas {
    Unavailable(String),
    Accounts(Vec<AccountQuotas>),
}

/// Percent of the limit still free, rounded to the nearest whole point.
pub fn percent_free(remaining: u64, limit: u64) -> u64 {
    ((u128::from(remaining) * 100 + u128::from(limit) / 2) / u128::from(limit)) as u64
}

/// Live subscription quota fetch shared by `/usage` and the status line.
/// Same client path as `/login`; never contacts anything when Weles is
/// unconfigured.
pub fn fetch_subscription_quotas() -> SubscriptionQuotas {
    let client = WelesClient::from_env();
    let health = client.health();
    if !health.available {
        return SubscriptionQuotas::Unavailable(health.detail);
    }
    let accounts = match client.accounts(None) {
        Ok(accounts) => accounts,
        Err(error) => return SubscriptionQuotas::Unavailable(error.to_string()),
    };
    if accounts.is_empty() {
        return SubscriptionQuotas::Unavailable("no Weles accounts (see /login)".into());
    }
    let mut out = Vec::new();
    for (account_index, account) in accounts.iter().enumerate() {
        let correlation = format!("usage-quota-{account_index}");
        let mut entries = Vec::new();
        match client.subscriptions(
            &account.id,
            &RequestMeta::read_v2(format!("{correlation}-subscriptions")),
        ) {
            Ok(subscriptions) => {
                let multiple_subscriptions = subscriptions.len() > 1;
                for (subscription_index, subscription) in subscriptions.iter().enumerate() {
                    let label = if multiple_subscriptions {
                        format!("{} · {}", account.display_name, subscription.product_id)
                    } else {
                        account.display_name.clone()
                    };
                    match client.quota(
                        &subscription.id,
                        &RequestMeta::read_v2(format!("{correlation}-quota-{subscription_index}")),
                    ) {
                        Ok(quota) => {
                            let multiple_buckets = quota.buckets.len() > 1;
                            for bucket in quota.buckets {
                                entries.push(QuotaEntry::Bucket(LabeledBucket {
                                    label: if multiple_buckets {
                                        format!("{label} · {}", bucket.bucket_id)
                                    } else {
                                        label.clone()
                                    },
                                    bucket,
                                }));
                            }
                        }
                        Err(error) => entries.push(QuotaEntry::Unavailable {
                            label,
                            error: error.to_string(),
                        }),
                    }
                }
            }
            Err(error) => entries.push(QuotaEntry::Unavailable {
                label: account.display_name.clone(),
                error: error.to_string(),
            }),
        }
        out.push(AccountQuotas {
            provider: account.provider.clone(),
            entries,
        });
    }
    SubscriptionQuotas::Accounts(out)
}

impl SubscriptionQuotas {
    /// Percent free of the most constrained reported bucket (minimum of
    /// remaining/limit across every bucket that reports both). `None` when
    /// nothing metered was reported, so callers can skip silently.
    pub fn min_percent_free(&self) -> Option<u64> {
        let Self::Accounts(accounts) = self else {
            return None;
        };
        accounts
            .iter()
            .flat_map(|account| account.entries.iter())
            .filter_map(|entry| {
                let QuotaEntry::Bucket(LabeledBucket { bucket, .. }) = entry else {
                    return None;
                };
                match (bucket.remaining, bucket.limit) {
                    (Some(remaining), Some(limit)) if limit > 0 => {
                        Some(percent_free(remaining, limit))
                    }
                    _ => None,
                }
            })
            .min()
    }
}
