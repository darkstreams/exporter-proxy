pub mod socket;

use std::{collections::HashMap, sync::Arc, time::SystemTime};

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use prettytable::{cell, row, Table};
use serde::Deserialize;
use tokio::sync::Semaphore;
use tracing::debug;

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
#[derive(Debug, Deserialize)]
pub struct MetricWithMeta(String, SystemTime, String);

/// A wrapper struct for easy use and to create methods
#[derive(Debug, Deserialize)]
pub struct GlobalMetrics {
    pub app_metrics: HashMap<String, MetricWithMeta>,
}

impl GlobalMetrics {
    pub fn new() -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            app_metrics: HashMap::new(),
        }))
    }

    pub fn insert(&mut self, app_name: String, metrics: String, source: String) {
        debug!(
            "inserting {}'s metrics {} received from {}",
            app_name, metrics, source
        );
        self.app_metrics
            .entry(app_name)
            .and_modify(|m| {
                *m = MetricWithMeta(
                    metrics.to_owned(),
                    std::time::SystemTime::now(),
                    source.to_owned(),
                )
            })
            .or_insert_with(|| MetricWithMeta(metrics, std::time::SystemTime::now(), source));
    }

    pub fn get_metrics(&self) -> String {
        let mut metrics = String::new();
        for (_, metric) in self.app_metrics.iter() {
            metrics.push_str(&metric.0);
        }
        metrics
    }
    pub fn get_app_names(&self) -> String {
        let mut current_time = Table::new();
        current_time.add_row(row![format!(
            "Current time: {}",
            DateTime::<Utc>::from(std::time::SystemTime::now())
        )]);
        let mut table = Table::new();
        table.add_row(row!["App Name", "Last Update Time (UTC)", "Source"]);
        for (app_name, metric) in self.app_metrics.iter() {
            let datetime = DateTime::<Utc>::from(metric.1);
            table.add_row(row![app_name, datetime, metric.2]);
        }
        current_time.add_row(row![table]);
        current_time.to_string()
    }
}
