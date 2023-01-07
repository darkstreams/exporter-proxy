pub mod socket;

use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use parking_lot::RwLock;
use prettytable::{row, Table};
// use prometheus::Histogram;
use anyhow::{Context, Result};
use time::OffsetDateTime;
use tokio::sync::Semaphore;
use tracing::{debug, error, info};

/// acquire permit
pub async fn acquire_permit(ratelimit: &Arc<Semaphore>) -> tokio::sync::OwnedSemaphorePermit {
    let permit = match ratelimit.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(err) => {
            panic!("error acquiring semaphore. This is a bug and shouldn't happen, as we're not dropping the semaphore. Raise an issue. Error: {}", err);
        }
    };
    permit
}

/// store metrics with the received epoch timestamp
#[derive(Debug)]
pub struct MetricWithMeta(String, SystemTime, String);

/// A wrapper struct for easy use and to create methods
#[derive(Debug)]
pub struct GlobalMetrics {
    pub polled_data: Stash<Vec<u8>>,
    pub capnp_data: Stash<String>,
    pub json_data: Stash<String>,
}

#[derive(Debug)]
pub struct Stash<T> {
    /// The generic metric value that's stored.
    inner: RwLock<Inner<T>>,
    // A histogram for lock contention metrics
    // lock_contention: Histogram,
}
#[derive(Debug)]
struct Inner<T> {
    /// The generic metric value that's stored.
    val: HashMap<String, T>,
    /// offset_ts will only be used to compute the expiry of the metrics.
    /// This will not be used to update the actual metric timestamp.
    offset_ts: Option<OffsetDateTime>,
    /// source of inbound
    source: T,
}

impl<T: AsRef<[u8]> + std::clone::Clone + std::default::Default> Stash<T> {
    // pub fn new(name: &str, help: &str, labels: Arc<HashMap<String, String>>) -> Self {
    pub fn new(name: &str, help: &str) -> Self {
        Self {
            inner: RwLock::new(Inner {
                val: Default::default(),
                offset_ts: None,
                source: Default::default(),
            }),
            // lock_contention: new_histogram(name, help, &[], vec![0.001, 0.1, 1.0, 5.0], labels)
            //     .with_label_values(&[]),
        }
    }

    fn get_inner_vals(&self) -> HashMap<String, T> {
        let guard = self.inner.read();
        let vals = guard.val.clone();
        drop(guard);
        vals
    }
    fn get_ts(&self) -> Option<OffsetDateTime> {
        let guard = self.inner.read();
        let ts = guard.offset_ts;
        drop(guard);
        ts
    }
    fn get_source(&self) -> String {
        let guard = self.inner.read();
        let src = guard.source.as_ref().to_owned();
        let t = std::str::from_utf8(&src).unwrap_or("cannot decode source");
        drop(guard);
        t.to_owned()
    }
    fn get_metrics(&self) -> String {
        let guard = self.inner.read();
        let mut metrics = String::new();
        for (k, v) in guard.val.iter() {
            let val = match std::str::from_utf8(v.as_ref()) {
                Ok(s) => s,
                Err(err) => {
                    error!(
                        "utf8 error, not formatting metric for {}, error: {}",
                        k, err
                    );
                    continue;
                }
            };
            metrics.push_str(val)
        }
        drop(guard);
        metrics
    }

    pub fn get<S: AsRef<str>>(&self, key: S) -> Option<T> {
        // let ct = coarsetime::Instant::now();
        let guard = self.inner.read();
        let val = guard.val.get(key.as_ref()).cloned();
        drop(guard);
        // let elapsed = ct.elapsed();
        // self.lock_contention.observe(elapsed.as_f64());
        val
    }
    pub fn discard_expired(&self, now_offset_ts: OffsetDateTime, stale_metric_ttl: i64) {
        // let ct = coarsetime::Instant::now();
        let mut guard = self.inner.write();
        let ts = guard.offset_ts;
        guard.val.retain(|k, _v| {
            if let Some(offset_ts) = ts {
                let diff = (now_offset_ts - offset_ts).whole_seconds();
                if diff < stale_metric_ttl {
                    info!(
                        "metrics {} is {} seconds old, but stale threshold is {} seconds and will not be cleared",
                        k, diff, stale_metric_ttl
                    );
                } else {
                    info!(
                        "metrics {} is {} seconds old, but stale threshold is {} seconds and will be cleared",
                        k, diff, stale_metric_ttl
                    );
                };
                diff < stale_metric_ttl
            } else {
                info!("no offset ts found, clearing metrics {}", k);
                false
            }
        });
        drop(guard);
        // let elapsed = ct.elapsed();
        // self.lock_contention.observe(elapsed.as_f64());
    }

    pub fn push(&self, key: &str, val: T, source: T, timestamp: Option<OffsetDateTime>) {
        let ts = match timestamp {
            Some(ts) => ts,
            None => match get_epoch_now_as_offset_dt() {
                Ok(now) => now,
                Err(err) => {
                    error!("error inserting timestamp: {}", err);
                    return;
                }
            },
        };
        // let ct = coarsetime::Instant::now();
        let mut guard = self.inner.write();
        guard.offset_ts = Some(ts);
        guard.val.insert(key.to_owned(), val);
        guard.source = source;
        debug!("inserted {} to stash", key);
        drop(guard);
        // let elapsed = ct.elapsed();
        // self.lock_contention.observe(elapsed.as_f64());
    }
}

impl GlobalMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            // app_metrics: HashMap::new(),
            polled_data: Stash::new("polled_data", "data scraped from external polls"),
            capnp_data: Stash::new("capnp_data", "data received as capnp"),
            json_data: Stash::new("json_data", "data received as json"),
        })
    }

    /// clean up stale metrics
    pub async fn scavenge(self: Arc<Self>, stale_check_interval: i64, stale_metric_ttl: i64) {
        let stale_check_interval_u64: u64 = match stale_check_interval.try_into() {
            Ok(interval) => interval,
            Err(err) => {
                error!("cannot run async task scavenger, error: {}", err);
                return;
            }
        };

        info!(
            "running gc to clear metrics older than {} seconds every {} seconds",
            stale_metric_ttl, stale_check_interval
        );
        // start running cleanup
        loop {
            debug!("sleeping for {} secs", stale_check_interval_u64);
            tokio::time::sleep(std::time::Duration::from_secs(stale_check_interval_u64)).await;

            let now_offset_ts = match get_epoch_now_as_offset_dt() {
                Ok(ts) => ts,
                Err(err) => {
                    error!("error getting current offset date time, error: {}", err);
                    continue;
                }
            };

            // purging stale sandbox metrics
            self.polled_data
                .discard_expired(now_offset_ts, stale_metric_ttl);
            self.capnp_data
                .discard_expired(now_offset_ts, stale_metric_ttl);
            self.json_data
                .discard_expired(now_offset_ts, stale_metric_ttl);
        }
    }

    pub fn get_app_names(&self) -> String {
        let mut current_time = Table::new();
        current_time.add_row(row![format!(
            "Current time: {}",
            get_epoch_now_as_offset_dt().unwrap_or_else(|_| { OffsetDateTime::now_utc() })
        )]);
        let mut table = Table::new();
        table.add_row(row!["App Name", "Last Update Time", "Source"]);
        for (app_name, _) in self.capnp_data.get_inner_vals() {
            table.add_row(row![
                app_name,
                self.capnp_data
                    .get_ts()
                    .unwrap_or_else(OffsetDateTime::now_utc),
                self.capnp_data.get_source()
            ]);
        }

        for (app_name, _) in self.json_data.get_inner_vals() {
            table.add_row(row![
                app_name,
                self.json_data
                    .get_ts()
                    .unwrap_or_else(OffsetDateTime::now_utc),
                self.json_data.get_source()
            ]);
        }

        for (app_name, _) in self.polled_data.get_inner_vals() {
            table.add_row(row![
                app_name,
                self.polled_data
                    .get_ts()
                    .unwrap_or_else(OffsetDateTime::now_utc),
                self.polled_data.get_source()
            ]);
        }

        current_time.add_row(row![table]);
        current_time.to_string()
    }

    pub fn get_metrics(&self) -> String {
        let mut metrics = String::new();
        metrics.push_str(&self.polled_data.get_metrics());
        metrics.push_str(&self.capnp_data.get_metrics());
        metrics.push_str(&self.json_data.get_metrics());
        metrics
    }
}

pub fn get_epoch_now_as_offset_dt() -> Result<OffsetDateTime> {
    let now = SystemTime::now();
    let now_epoch = i64::try_from(
        now.duration_since(UNIX_EPOCH)
            .context("system time error")?
            .as_secs(),
    )?;
    OffsetDateTime::from_unix_timestamp(now_epoch)
        .context("cannot convert current unix timestamp to offset time")
}
