# A dumb light weight asynchronous exporter proxy

This is a dumb lightweight asynchronous exporter proxy that will help to expose multiple application metrics over a single port. This will be helpful if it is difficult to open multiple ports because of firewall restrictions.

Exporter proxy is capable of receiving and polling.
 
Exporter proxy receives data either as capnp or as json. It will not accept both of them at the same time. Check the `receiver.format` section in the config.toml to choose one.

If multiple applications send metrics to the exporter proxy, it is the application's responsibility to add a prefix to the metric name to prevent collission. If there is a duplicate occurrence of same metric name from two different applications, the exporter proxy will expose both occurrences. 

Both the TCP and the Unix socket receiver endpoints expects the data to be in a key-value form. The capnp schema is also a basic kv structure where the key is the name of the application that is sending the metrics and the value, a string representation of metrics. If the format is chosen as json, it could be either a single multiline string representation of the entire metrics, or a json array strings. This is just a way for the exporter proxy to distinguish between the metrices send by different application. Upon receiving a new set of metrics, the old ones exposed will be overwritten. 

Eg: if at first, an application sends the following metrics

```bash
"http_requests_total_foo_bar_app1{method=\"post\",code=\"200\"} 1027 1395066363000",
"http_requests_total_foo_bar_app1{method=\"post\",code=\"400\"}    3 1395066363000"
```

and then it only sends

```bash
"http_requests_total_foo_bar_app1{method=\"post\",code=\"200\"} 1029 1395066363000",
```
the exporter proxy will only have

```bash
"http_requests_total_foo_bar_app1{method=\"post\",code=\"200\"} 1029 1395066363000",
```
and will not remember `"http_requests_total_foo_bar_app1{method=\"post\",code=\"400\"}    3 1395066363000"`

## Endpoints

- a scrape endpoint `/metrics`
this exposes all the metrics combined. As mentioned earlier, this is a dumb proxy and will not validate the prometheus exposition format. It is the application's responsibility to validate it before sending it. Most prometheus libraries supports formatting the metrics in a valid exposition format.
- a metadata endpoint `/apps`
this exposes a list of apps that sends metrics, the source that received the metrics and the last received time in UTC

```bash
exporter-proxy/python_test_clients  🍣 master 🐍 v3.9.2 🐏 6GiB/8GiB | 1024MiB/1024MiB
✦3 🕙 23:51:49 ⬢ [Docker] ✖  curl localhost:6555/apps
+---------------------------------------------------------------------------------------------------------------+
| Current time: 2022-05-22 23:51:52.939898700 UTC                                                               |
+---------------------------------------------------------------------------------------------------------------+
| +-------------------------------+-----------------------------------+---------------------------------------+ |
| | App Name                      | Last Update Time (UTC)            | Source                                | |
| +-------------------------------+-----------------------------------+---------------------------------------+ |
| | external_poll: 127.0.0.1:1027 | 2022-05-22 23:51:47.971181300 UTC | external_poll: 127.0.0.1:1027         | |
| +-------------------------------+-----------------------------------+---------------------------------------+ |
| | external_poll: 127.0.0.1:1025 | 2022-05-22 23:51:47.970590700 UTC | external_poll: 127.0.0.1:1025         | |
| +-------------------------------+-----------------------------------+---------------------------------------+ |
| | foo_bar_app1_metrics          | 2022-05-22 23:51:07.406709200 UTC | /var/run/exporter-proxy-receiver.sock | |
| +-------------------------------+-----------------------------------+---------------------------------------+ |
| | external_poll: 127.0.0.1:1026 | 2022-05-22 23:51:47.970724 UTC    | external_poll: 127.0.0.1:1026         | |
| +-------------------------------+-----------------------------------+---------------------------------------+ |
| | foo_bar_app0_metrics          | 2022-05-22 23:51:42.737333400 UTC | 127.0.0.1:6554                        | |
| +-------------------------------+-----------------------------------+---------------------------------------+ |
+---------------------------------------------------------------------------------------------------------------+

```

## Test sending metrics

These tests assume the default configuration as mentioned in the `config.toml`. If it is modified, change accordingly.
A couple of json files and a python script is included in the [python_test_clients](./python_test_clients) directory.
`json_string.json` has the metric data as a single string whereas the `json_array.json` has the metric data as an array of strings. These are the two schemas that are supported. If the metrics isn't in any of these json format, it will not be deserialized and proxied.


run the exporter proxy as

```bash
RUST_LOG="trace" cargo run
```

and `cd python_test_clients` from another shell.

To proxy the capnp serialized test metrics, first make sure that the `receiver.format` is `capnp` and not `json`. Restart the exporter proxy if it was already running.

-  install the capnp library

```bash
pip install -r requirements.txt
```

- run the script `test_send_tcp_socket_capnp.py` and `curl localhost:6555/metrics` to see if it is exposing the metrics in prometheus exposition format. Run `curl localhost:6555/apps` to see the app names that sent the metrics.

To proxy the the json serialized message, make sure that the `receiver.format` is `json` and not `capnp`.

- install netcat-openbsd

- run the test with either of the json format as

```bash
# to send it over the unix socket
cat ./json_array.json  | nc -U /var/run/exporter-proxy-receiver.sock -q 0
```

or 

```bash
# to send it over tcp socket
cat ./json_string.json | nc -s 127.0.0.1 127.0.0.1 6554 -q 0
```

To test the external poll, cd to `python_test_clients` directory and run 

```
python3 -m http.server 1025&
python3 -m http.server 1026&
python3 -m http.server 1027&
```

to run 3 instances of webservers on port 1025, 1026 and 1027 respectively.

After this, the metrics can be checked by `cURL`ing the endpoints mentioned in the `config.toml`'s `poll_external.other_scrape_endpoints` section like

```bash
curl localhost:1026/1026.html
```

