import capnp
import src.schema.metric_data_capnp as metric_data_capnp
import socket
import sys
metrices = []

temp_metrics0 = metric_data_capnp.InboundMetrics.new_message()
temp_metrics0.appName = "foo_bar_app0"
temp_metrics0.data = '''
# HELP http_requests_total_foo_bar_app0 The total number of HTTP requests.
# TYPE http_requests_total_foo_bar_app0 counter
http_requests_total_foo_bar_app0{method="post",code="200"} 1027 1395066363000
http_requests_total_foo_bar_app0{method="post",code="400"}    3 1395066363000
'''

temp_metrics1 = metric_data_capnp.InboundMetrics.new_message()
temp_metrics1.appName = "foo_bar_app1"
temp_metrics1.data = '''
# HELP http_requests_total_foo_bar_app1 The total number of HTTP requests.
# TYPE http_requests_total_foo_bar_app1 counter
http_requests_total_foo_bar_app1{method="post",code="200"} 1027 1395066363000
http_requests_total_foo_bar_app1{method="post",code="400"}    3 1395066363000
'''

metrices.append(temp_metrics0)
metrices.append(temp_metrics1)



sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
socket_path = "/var/run/exporter-proxy-receiver.sock"
try:
    sock.connect(socket_path)
except socket.error as e:
    print(e)
    sys.exit(1)
print("inserting {}".format(metrices))
try:
    for i in metrices:
        i.write(sock)
except Exception as e:
    print(e)
    sys.exit(1)