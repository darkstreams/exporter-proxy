mod args;
mod inbound;
mod poll_external;
mod schema;
mod scrape;

use std::sync::Arc;

use args::{Args, Config};

use anyhow::{Context, Result};
use tokio::{runtime, sync::Semaphore};
use tracing::{debug, info};

use crate::{
    args::Format::{CapnP, Json},
    inbound::{
        socket::{
            create_tcp_listener, create_unix_listener, run_tcp_listener_capnp,
            run_tcp_listener_json, run_unix_listener_capnp, run_unix_listener_json,
        },
        GlobalMetrics,
    },
};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let arg = Args::new()?;
    let parsed = arg.parse_args();
    let config_file_path = parsed.value_of("config").context("invalid config")?;
    info!("parsing {}", config_file_path);
    let config = Config::from_path(config_file_path)?;
    debug!("building runtime from {:?}", config);
    let rt = match config.exporter.worker_threads {
        0 => {
            info!("creating current_thread runtime");
            runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("cannot create runtime")?
        }
        multi => {
            info!("starting multi-threaded with {} worker threads", multi);
            runtime::Builder::new_multi_thread()
                .worker_threads(multi)
                .max_blocking_threads(multi)
                .enable_all()
                .thread_name("resolver-core")
                .build()
                .context("cannot create runtime")?
        }
    };

    /* Get a handle to the runtime */
    let handle = rt.handle();
    /* Enter the runtime (required because the create_unix_listener will panic as it uses
        tokio::net::UnixListener::bind that requires a reactor running in the context of tokio 1.x runtime)
    */
    let _enterguard = handle.enter();

    // Create a global metric
    let global_metrics = GlobalMetrics::new();
    debug!("created global metric instance {:?}", global_metrics);

    // start listening on unix socket if enabled
    if config.receiver.receive_on_unix_socket {
        info!(
            "unix socket enabled, listening on {}",
            config.receiver.receiver_unix_socket
        );
        let socket = create_unix_listener(
            &config.receiver.receiver_unix_socket,
            &config.receiver.unix_socket_user,
            &config.receiver.unix_socket_group,
        )?;
        let unix_limit = Arc::new(Semaphore::new(config.exporter.concurrent_requests.into()));

        match config.receiver.format {
            CapnP => {
                rt.spawn(run_unix_listener_capnp(
                    socket,
                    global_metrics.clone(),
                    config.receiver.receiver_unix_socket,
                    unix_limit,
                ));
            }
            Json => {
                rt.spawn(run_unix_listener_json(
                    socket,
                    global_metrics.clone(),
                    config.receiver.receiver_unix_socket,
                    unix_limit,
                ));
            }
        }
    }

    // start listening on TCP socket if enabled
    if config.receiver.receive_on_tcp_socket {
        let socket = rt.block_on(create_tcp_listener(config.receiver.receiver_tcp_socket))?;
        let tcp_limit = Arc::new(Semaphore::new(config.exporter.concurrent_requests.into()));

        match config.receiver.format {
            CapnP => {
                rt.spawn(run_tcp_listener_capnp(
                    socket,
                    global_metrics.clone(),
                    config.receiver.receiver_tcp_socket,
                    tcp_limit,
                ));
            }
            Json => {
                rt.spawn(run_tcp_listener_json(
                    socket,
                    global_metrics.clone(),
                    config.receiver.receiver_tcp_socket,
                    tcp_limit,
                ));
            }
        }
    }

    // start to poll external metrics if enabled
    if config.poll_external.poll_other_scrape_endpoints {
        rt.spawn(poll_external::poll_external(
            config.poll_external.other_scrape_endpoints,
            global_metrics.clone(),
            config.poll_external.poll_interval,
            config.poll_external.client_timeout,
            config.poll_external.allow_invalid_certs,
        ));
    }

    rt.block_on(scrape::start_warp(
        config.exporter.scrape_expose_endpoint,
        global_metrics,
    ));
    Ok(())
}
