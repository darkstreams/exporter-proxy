use std::{collections::HashMap, net::SocketAddr, path::Path, sync::Arc};

use crate::inbound::acquire_permit;

use super::GlobalMetrics;
use anyhow::{anyhow, Context, Result};
use capnp_futures;
use exporter_proxy::schema::metric_data_capnp;
use libc::{self, gid_t, uid_t};
use nix::{errno::Errno, NixPath};
use tokio::{
    io::{AsyncReadExt, BufStream},
    net::{TcpListener, UnixListener},
    sync::Semaphore,
};
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::{debug, error, info, trace};
use users::{get_current_uid, get_group_by_name, get_user_by_name, get_user_by_uid};

/// Create a unix socket from the socket path and chowns that socket to the socket user and group
pub fn create_unix_listener<T: AsRef<str>>(
    sock_path: T,
    user: Option<T>,
    group: Option<T>,
) -> Result<UnixListener> {
    let socket_path = Path::new(sock_path.as_ref());
    let _ = std::fs::remove_file(socket_path);
    let user = match user {
        Some(usr) => get_user_by_name(usr.as_ref()).ok_or_else(|| anyhow!("user not found"))?,
        None => get_user_by_uid(get_current_uid()).context("cannot find current user")?,
    };
    let group = match group {
        Some(grp) => get_group_by_name(grp.as_ref()).ok_or_else(|| anyhow!("group not found"))?,
        None => {
            let user = get_user_by_uid(get_current_uid()).context("cannot find current user")?;
            users::get_group_by_gid(user.primary_group_id())
                .context("cannot find primary group of current user")?
        }
    };
    let listener = UnixListener::bind(sock_path.as_ref())?;
    let res = socket_path.with_nix_path(|cstr| unsafe {
        libc::chown(cstr.as_ptr(), user.uid() as uid_t, group.gid() as gid_t)
    })?;
    Errno::result(res)?;
    info!(
        "Starting to listen on unix socket at {:?}",
        sock_path.as_ref()
    );
    Ok(listener)
}

/// Create a TCP listener
pub async fn create_tcp_listener(addr: SocketAddr) -> Result<TcpListener> {
    info!("Starting to listen on TCP socket at {:?}", addr);
    TcpListener::bind(addr)
        .await
        .context(format!("cannot create TCP listener on {:?}", addr))
}

/// handles requests from unix socket that are supposed to be in capnp format.
pub async fn run_unix_listener_capnp(
    listener: UnixListener,
    metrics: Arc<GlobalMetrics>,
    sourceaddr: String,
    ratelimit: Arc<Semaphore>,
) {
    loop {
        let permit = acquire_permit(&ratelimit).await;
        debug!("acquired semaphore, {:?}", permit);
        match listener.accept().await {
            Ok((stream, _)) => {
                debug!("Received unix socket connection for capnp handler");
                let metrics_clone = metrics.clone();
                let sourceaddr_clone = sourceaddr.clone();
                tokio::task::spawn(async move {
                    handle_client_capnp(stream.compat(), metrics_clone, sourceaddr_clone).await;
                    drop(permit);
                });
            }
            Err(err) => {
                error!(
                    "error accepting on unix socket capnp handler, yielding task. err: {}",
                    err
                );
                tokio::task::yield_now().await;
                drop(permit);
            }
        }
    }
}

/// A generic function that accepts both unix stream and tcp stream and deserialize the capnp data
pub async fn handle_client_capnp<
    T: std::marker::Unpin + FuturesAsyncReadCompatExt + std::fmt::Debug,
>(
    mut stream: T,
    metrics: Arc<GlobalMetrics>,
    source: String,
) {
    loop {
        let deserialized =
            match capnp_futures::serialize::try_read_message(&mut stream, Default::default()).await
            {
                Ok(reader) => match reader {
                    Some(des) => des,
                    None => {
                        debug!("none received");
                        return;
                    }
                },
                Err(err) => {
                    error!("{}", err.to_string());
                    return;
                }
            };
        // get root of the capnp message
        let root = match deserialized.get_root::<metric_data_capnp::inbound_metrics::Reader>() {
            Ok(root) => root,
            Err(err) => {
                error!("{}", err.to_string());
                return;
            }
        };

        // get app_name and app_data
        let app_name = match root.get_app_name() {
            Ok(name) => name,
            Err(err) => {
                error!("{}", err.to_string());
                return;
            }
        };
        let app_data = match root.get_data() {
            Ok(data) => data,
            Err(err) => {
                error!("{}", err.to_string());
                return;
            }
        };
        // insert to global metrics
        // let mut guard = metrics.write();
        metrics
            .capnp_data
            .push(app_name, app_data.to_owned(), source.to_owned(), None);
    }
}

pub async fn run_unix_listener_json(
    listener: UnixListener,
    metrics: Arc<GlobalMetrics>,
    sourceaddr: String,
    ratelimit: Arc<Semaphore>,
) {
    loop {
        let permit = acquire_permit(&ratelimit).await;
        debug!("acquired semaphore, {:?}", permit);
        match listener.accept().await {
            Ok((stream, _)) => {
                debug!("Received unix socket connection for json handler");
                let metrics_clone = metrics.clone();
                let sourceaddr_clone = sourceaddr.clone();
                tokio::task::spawn(async move {
                    handle_client_json(stream, metrics_clone, sourceaddr_clone).await;
                    drop(permit);
                });
            }
            Err(err) => {
                error!(
                    "error accepting on unix socket json handler, yielding task. err: {}",
                    err
                );
                tokio::task::yield_now().await;
                drop(permit);
            }
        }
    }
}

/// A generic function that accepts both unix stream and tcp stream and deserialize the json data
pub async fn handle_client_json<
    T: std::marker::Unpin + std::fmt::Debug + tokio::io::AsyncRead + tokio::io::AsyncWrite,
>(
    mut stream: T,
    metrics: Arc<GlobalMetrics>,
    source: String,
) {
    loop {
        let mut buf_stream = BufStream::new(&mut stream);
        let mut data = Vec::new();
        match buf_stream.read_to_end(&mut data).await {
            Ok(0) => {
                trace!("read till eof");
                break;
            }
            Ok(b) => {
                debug!("{} bytes read", b);
            }
            Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(err) => {
                error!("error reading to buf, error: {}", err);
                return;
            }
        }
        match serde_json::from_slice::<HashMap<String, String>>(&data) {
            Ok(mapped) => {
                if let Some((k, v)) = mapped.into_iter().next() {
                    metrics.json_data.push(&k, v, source, None);
                    return;
                }
            }
            Err(err) => {
                error!("invalid data, error: {}, trying a fallback type", err);
                match serde_json::from_slice::<HashMap<String, Vec<String>>>(&data) {
                    Ok(mapped) => {
                        if let Some((k, v)) = mapped.into_iter().next() {
                            let mut metric_joined = v.join("\n");
                            metric_joined.push('\n');
                            metrics.json_data.push(&k, metric_joined, source, None);
                            return;
                        }
                    }
                    Err(err) => {
                        error!(
                            "cannot parse with fallback type. Invalid data, error: {},",
                            err
                        );
                        return;
                    }
                }
            }
        };
    }
}

pub async fn run_tcp_listener_capnp(
    listener: TcpListener,
    metrics: Arc<GlobalMetrics>,
    sourceaddr: SocketAddr,
    ratelimit: Arc<Semaphore>,
) {
    loop {
        let permit = acquire_permit(&ratelimit).await;
        debug!("acquired semaphore, {:?}", permit);
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(
                    "Received tcp socket connection for capnp handler from {}",
                    addr
                );
                let metrics_clone = metrics.clone();
                tokio::task::spawn(async move {
                    handle_client_capnp(stream.compat(), metrics_clone, sourceaddr.to_string())
                        .await;
                    drop(permit);
                });
            }
            Err(err) => {
                error!(
                    "error accepting on tcp socket capnp handler, yielding task. err: {}",
                    err
                );
                tokio::task::yield_now().await;
                drop(permit);
            }
        }
    }
}

pub async fn run_tcp_listener_json(
    listener: TcpListener,
    metrics: Arc<GlobalMetrics>,
    sourceaddr: SocketAddr,
    ratelimit: Arc<Semaphore>,
) {
    loop {
        let permit = acquire_permit(&ratelimit).await;
        debug!("acquired semaphore, {:?}", permit);
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(
                    "Received tco socket connection for json handler from {}",
                    addr
                );
                let metrics_clone = metrics.clone();
                tokio::task::spawn(async move {
                    handle_client_json(stream, metrics_clone, sourceaddr.to_string()).await;
                    drop(permit);
                });
            }
            Err(err) => {
                error!(
                    "error accepting on tcp socket json handler, yielding task. err: {}",
                    err
                );
                tokio::task::yield_now().await;
                drop(permit);
            }
        }
    }
}
