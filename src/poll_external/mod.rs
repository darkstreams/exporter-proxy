use std::sync::Arc;

use tracing::{error, info};
use url::Url;

use crate::inbound::GlobalMetrics;

pub async fn poll_external(
    endpoints: Vec<Url>,
    global_metrics: Arc<GlobalMetrics>,
    poll_interval: u64,
    client_timeout: u64,
    accept_invalid_cert: bool,
) {
    let client = match reqwest::Client::builder()
        .timeout(tokio::time::Duration::from_secs(client_timeout))
        .danger_accept_invalid_certs(accept_invalid_cert)
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            panic!(
                "cannot initialize the http client to poll endpoints, err: {}",
                err
            )
        }
    };
    info!(
        "polling external endpoints {:?} every {} secs",
        &endpoints, poll_interval
    );
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(poll_interval)).await;

        let mut gather_futures = Vec::new();

        for endpoint in endpoints.iter() {
            let resp = tokio::spawn(client.get(endpoint.to_owned()).send());
            gather_futures.push(resp)
        }

        for task in gather_futures {
            let resp = match task.await {
                Ok(res) => match res {
                    Ok(response) => response,
                    Err(err) => {
                        error!(
                            "error scraping endpoint, discarding this request. Error: {}",
                            err
                        );
                        continue;
                    }
                },
                Err(err) => {
                    error!(
                        "error joining future, discarding this request. Error: {}",
                        err
                    );
                    continue;
                }
            };
            // Todo: Add a metric endpoint here
            if resp.status() != 200 {
                error!("status code not 200, discarding this request");
                continue;
            }
            let remote_addr = match resp.remote_addr() {
                Some(addr) => format!("external_poll: {}", addr),
                None => {
                    error!("error finding remote address, discarding this request");
                    continue;
                }
            };
            let data = match resp.bytes().await {
                Ok(data) => {
                    let mut body = data.to_vec();
                    body.push(b'\n'); // Add a newline regardless
                    body
                }
                Err(err) => {
                    error!(
                        "error reading response, discarding this request. Error: {}",
                        err
                    );
                    continue;
                }
            };

            global_metrics.polled_data.push(
                &remote_addr.to_owned(),
                data,
                remote_addr.into_bytes(),
                None,
            )
        }
    }
}
