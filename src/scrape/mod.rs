use std::{net::SocketAddr, sync::Arc};

use tracing::info;
use warp::Filter;

use crate::inbound::GlobalMetrics;

fn get_routes(metrics: Arc<GlobalMetrics>) -> impl Filter<Extract = (impl warp::Reply,)> + Clone {
    let metrics_clone = metrics.clone();
    let metrics_route = warp::path("metrics")
        .and(warp::get())
        .and(warp::path::end())
        .and_then(move || {
            let metricsclone_clone = metrics_clone.clone();
            serve_metrics(metricsclone_clone)
        });

    let index = warp::path::end().and(warp::get()).and_then(get_index);
    let app_names = warp::path("apps")
        .and(warp::get())
        .and(warp::path::end())
        .and_then(move || {
            let metrics = metrics.clone();
            get_app_names(metrics)
        });
    let log = warp::log("warp::access_log");

    metrics_route.or(app_names).or(index).with(log)
}

pub async fn start_warp(addr: SocketAddr, metrics: Arc<GlobalMetrics>) {
    let routes = get_routes(metrics);

    warp::serve(routes).run(addr).await;
}

async fn serve_metrics(metrics: Arc<GlobalMetrics>) -> Result<String, warp::Rejection> {
    Ok(metrics.get_metrics())
}

async fn get_app_names(metrics: Arc<GlobalMetrics>) -> Result<String, warp::Rejection> {
    Ok(metrics.get_app_names())
}

async fn get_index() -> Result<String, warp::Rejection> {
    Ok("home_page".to_string())
}
