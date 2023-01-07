use std::{fs::File, io::Read, net::SocketAddr, path::Path};

use anyhow::{Context, Result};
use clap::{Arg, ArgMatches, Command};
use serde::Deserialize;
use url::Url;

pub(crate) struct Args<'a> {
    pub(crate) version: &'a str,
    pub(crate) author: &'a str,
    pub(crate) about: &'a str,
    pub(crate) default_config_file: String,
}

impl Args<'_> {
    pub(crate) fn new() -> Result<Self> {
        Ok(Args {
            version: env!("CARGO_PKG_VERSION"),
            author: env!("CARGO_PKG_AUTHORS"),
            about: "A lightweight asynchronous exporter proxy that receives data over a unix socket in Capn'Proto, combines everything and exposes it as an http scrape endpoint",
            default_config_file: format!(
                "{}/config.toml",
                env!("CARGO_MANIFEST_DIR")
            ),
        })
    }
    pub(crate) fn parse_args(&self) -> ArgMatches {
        Command::new("exporter-proxy")
            .version(self.version)
            .author(self.author)
            .about(self.about)
            .arg(
                Arg::new("config")
                    .long("config")
                    .short('c')
                    .help("path to the config file")
                    .takes_value(true)
                    .default_value(&self.default_config_file),
            )
            .get_matches()
    }
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub exporter: Exporter,
    pub receiver: Receiver,
    pub poll_external: Option<PollExternal>,
}

#[derive(Debug, Deserialize)]
pub struct Receiver {
    pub capnp: Option<CapnP>,
    pub json: Option<Json>,
}
#[derive(Debug, Deserialize)]
pub struct CapnP {
    pub receiver_unix_socket: Option<String>,
    pub unix_socket_user: Option<String>,
    pub unix_socket_group: Option<String>,
    pub receiver_tcp_socket: Option<SocketAddr>,
}

#[derive(Debug, Deserialize)]
pub struct Json {
    pub receiver_unix_socket: Option<String>,
    pub unix_socket_user: Option<String>,
    pub unix_socket_group: Option<String>,
    pub receiver_tcp_socket: Option<SocketAddr>,
}

#[derive(Debug, Deserialize)]
pub struct PollExternal {
    pub other_scrape_endpoints: Vec<Url>,
    pub poll_interval: u64,
    pub client_timeout: u64,
    pub allow_invalid_certs: bool,
}

#[derive(Debug, Deserialize)]
pub struct Exporter {
    pub worker_threads: usize,
    pub concurrent_requests: u16,
    pub scrape_expose_endpoint: SocketAddr,
}

impl Config {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Config> {
        let mut fd = File::open(path)?;
        let mut toml = String::new();
        fd.read_to_string(&mut toml)?;
        Self::from_string(&toml)
    }

    pub fn from_string(toml: &str) -> Result<Self> {
        let c: Config = toml::from_str(toml).context("context")?;
        Ok(c)
    }
}

#[derive(Debug, Deserialize)]
pub enum Format {
    #[serde(rename = "capnp")]
    CapnP,
    #[serde(rename = "json")]
    Json,
}
