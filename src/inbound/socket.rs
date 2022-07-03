use std::{collections::HashMap, net::SocketAddr, path::Path, sync::Arc};

use super::GlobalMetrics;
use anyhow::{anyhow, Context, Result};
use capnp_futures;
use exporter_proxy::schema::metric_data_capnp;
use libc::{self, gid_t, uid_t};
use nix::{errno::Errno, NixPath};
use parking_lot::RwLock;
use tokio::{
    io::{AsyncReadExt, BufStream},
    net::{TcpListener, UnixListener},
};
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::{debug, error, info, trace};
use users::{get_current_uid, get_group_by_name, get_user_by_name, get_user_by_uid};

/// Create a unix socket from the socket path and chowns that socket to the socket user and group
pub fn create_unix_listener(sock_path: &str, user: &str, group: &str) -> Result<UnixListener> {
    let socket_path = Path::new(sock_path);
    let _ = std::fs::remove_file(socket_path);
    let user = match user {
        "current_user" => get_user_by_uid(get_current_uid()).context("cannot find current user")?,
        anything_else => {
            get_user_by_name(anything_else).ok_or_else(|| anyhow!("user not found"))?
        }
    };
    let group = match group {
        "current_group" => {
            let user = get_user_by_uid(get_current_uid()).context("cannot find current user")?;
            users::get_group_by_gid(user.primary_group_id())
                .context("cannot find primary group of current user")?
        }
        anything_else => {
            get_group_by_name(anything_else).ok_or_else(|| anyhow!("group not found"))?
        }
    };
    let listener = UnixListener::bind(sock_path)?;
    let res = socket_path.with_nix_path(|cstr| unsafe {
        libc::chown(cstr.as_ptr(), user.uid() as uid_t, group.gid() as gid_t)
    })?;
    Errno::result(res)?;
    info!("Starting to listen on unix socket at {:?}", sock_path);
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
    metrics: Arc<RwLock<GlobalMetrics>>,
    sourceaddr: String,
) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                debug!("Received unix socket connection for capnp handler");
                tokio::task::spawn(handle_client_capnp(
                    stream.compat(),
                    metrics.clone(),
                    sourceaddr.to_owned(),
                ));
            }
            Err(err) => {
                error!(
                    "error accepting on unix socket capnp handler, yielding task. err: {}",
                    err
                );
                tokio::task::yield_now().await;
            }
        }
    }
}

/// A generic function that accepts both unix stream and tcp stream and deserialize the capnp data
pub async fn handle_client_capnp<
    T: std::marker::Unpin + FuturesAsyncReadCompatExt + std::fmt::Debug,
>(
    mut stream: T,
    metrics: Arc<RwLock<GlobalMetrics>>,
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
        let mut guard = metrics.write();
        guard.insert(
            app_name.to_string(),
            app_data.to_string(),
            source.to_owned(),
        );
        drop(guard);
    }
}

pub async fn run_unix_listener_json(
    listener: UnixListener,
    metrics: Arc<RwLock<GlobalMetrics>>,
    source: String,
) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                debug!("Received unix socket connection for json handler");
                tokio::task::spawn(handle_client_json(
                    stream,
                    metrics.clone(),
                    source.to_owned(),
                ));
            }
            Err(err) => {
                error!(
                    "error accepting on unix socket json handler, yielding task. err: {}",
                    err
                );
                tokio::task::yield_now().await;
            }
        }
    }
}

/// A generic function that accepts both unix stream and tcp stream and deserialize the json data
pub async fn handle_client_json<
    T: std::marker::Unpin + std::fmt::Debug + tokio::io::AsyncRead + tokio::io::AsyncWrite,
>(
    mut stream: T,
    metrics: Arc<RwLock<GlobalMetrics>>,
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
                    let mut guard = metrics.write();
                    guard.insert(k, v, source);
                    drop(guard);
                    return;
                }
            }
            Err(err) => {
                error!("invalid data, error: {}, trying a fallback type", err);
                match serde_json::from_slice::<HashMap<String, Vec<String>>>(&data) {
                    Ok(mapped) => {
                        if let Some((k, v)) = mapped.into_iter().next() {
                            let mut guard = metrics.write();
                            let mut metric_joined = v.join("\n");
                            metric_joined.push('\n');
                            guard.insert(k, metric_joined, source);
                            drop(guard);
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
    metrics: Arc<RwLock<GlobalMetrics>>,
    sourceaddr: SocketAddr,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(
                    "Received tcp socket connection for capnp handler from {}",
                    addr
                );
                tokio::task::spawn(handle_client_capnp(
                    stream.compat(),
                    metrics.clone(),
                    sourceaddr.to_string(),
                ));
            }
            Err(err) => {
                error!(
                    "error accepting on tcp socket capnp handler, yielding task. err: {}",
                    err
                );
                tokio::task::yield_now().await;
            }
        }
    }
}

pub async fn run_tcp_listener_json(
    listener: TcpListener,
    metrics: Arc<RwLock<GlobalMetrics>>,
    sourceaddr: SocketAddr,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(
                    "Received tco socket connection for json handler from {}",
                    addr
                );
                tokio::task::spawn(handle_client_json(
                    stream,
                    metrics.clone(),
                    sourceaddr.to_string(),
                ));
            }
            Err(err) => {
                error!(
                    "error accepting on tcp socket json handler, yielding task. err: {}",
                    err
                );
                tokio::task::yield_now().await;
            }
        }
    }
}
