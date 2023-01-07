// #[cfg(windows)]
// compile_error!("Not supporing on Windows at the moment because this uses unix sockets");
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

    // start listening for capnp events

    if let Some(capnp_receiver) = config.receiver.capnp {
        info!("capnp receiver enabled");

        // start listening for capnp unix socket events
        if let Some(capnp_unix_sock_path) = capnp_receiver.receiver_unix_socket {
            info!(
                "unix socket config found, spinning up async capnp receiver over unix socket at {}",
                capnp_unix_sock_path
            );

            let socket = create_unix_listener(
                &capnp_unix_sock_path,
                capnp_receiver.unix_socket_user.as_ref(),
                capnp_receiver.unix_socket_group.as_ref(),
            )?;
            let unix_limit = Arc::new(Semaphore::new(config.exporter.concurrent_requests.into()));
            rt.spawn(run_unix_listener_capnp(
                socket,
                global_metrics.clone(),
                capnp_unix_sock_path,
                unix_limit,
            ));
        }
        // start listening for capnp tcp events
        if let Some(capnp_tcp_sock) = capnp_receiver.receiver_tcp_socket {
            info!(
                "tcp socket config found, spinning up async capnp receiver over tcp at {}",
                capnp_tcp_sock
            );
            let socket = rt.block_on(create_tcp_listener(capnp_tcp_sock))?;
            let tcp_limit = Arc::new(Semaphore::new(config.exporter.concurrent_requests.into()));
            rt.spawn(run_tcp_listener_capnp(
                socket,
                global_metrics.clone(),
                capnp_tcp_sock,
                tcp_limit,
            ));
        }
    }

    // start listening for json events
    if let Some(json_receiver) = config.receiver.json {
        info!("json format receiver enabled");
        // start listening for json unix socket events
        if let Some(json_unix_sock_path) = json_receiver.receiver_unix_socket {
            info!(
                "unix socket config found, spinning up async json receiver over unix socket at {}",
                json_unix_sock_path
            );

            let socket = create_unix_listener(
                &json_unix_sock_path,
                json_receiver.unix_socket_user.as_ref(),
                json_receiver.unix_socket_group.as_ref(),
            )?;
            let unix_limit = Arc::new(Semaphore::new(config.exporter.concurrent_requests.into()));
            rt.spawn(run_unix_listener_capnp(
                socket,
                global_metrics.clone(),
                json_unix_sock_path,
                unix_limit,
            ));
        }
        if let Some(json_tcp_sock) = json_receiver.receiver_tcp_socket {
            info!(
                "tcp socket config found, spinning up async json receiver over tcp at {}",
                json_tcp_sock
            );
            let socket = rt.block_on(create_tcp_listener(json_tcp_sock))?;
            let tcp_limit = Arc::new(Semaphore::new(config.exporter.concurrent_requests.into()));
            rt.spawn(run_tcp_listener_capnp(
                socket,
                global_metrics.clone(),
                json_tcp_sock,
                tcp_limit,
            ));
        }
    }

    // start to poll external metrics if enabled
    if let Some(poll_external) = config.poll_external {
        rt.spawn(poll_external::poll_external(
            poll_external.other_scrape_endpoints,
            global_metrics.clone(),
            poll_external.poll_interval,
            poll_external.client_timeout,
            poll_external.allow_invalid_certs,
        ));
    }

    rt.block_on(scrape::start_warp(
        config.exporter.scrape_expose_endpoint,
        global_metrics,
    ));
    Ok(())
}
