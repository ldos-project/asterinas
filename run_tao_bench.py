import subprocess
import time
import socket

server_cmd = [
    "python3", "/usr/local/DCPerf/packages/tao_bench/run.py", "server",
    "--memsize", "10.0",
    "--nic-channel-ratio", "0.5",
    "--fast-threads-ratio", "0.75",
    "--dispatcher-to-fast-ratio", "0.25",
    "--slow-to-fast-ratio", "3.0",
    "--slow-threads-use-semaphore", "0",
    "--pin-threads", "0",
    "--interface-name", "lo",
    "--stats-interval", "5000",
    "--timeout-buffer", "0",
    "--warmup-time", "1200",
    "--test-time", "720",
    "--disable-tls", "0",
    "--smart-nanosleep", "0",
    "--port-number", "11211",
    "--real"
]

client_cmd = [
    "python3", "/usr/local/DCPerf/packages/tao_bench/run.py", "client",
    "--server-hostname", "127.0.0.1",
    "--server-memsize", "10.0",
    "--num-threads", "1",
    "--warmup-time", "1200",
    "--test-time", "720",
    "--disable-tls", "1",
    "--real"
]


server = subprocess.Popen(server_cmd)
time.sleep(60)

client = subprocess.Popen(client_cmd)
client.wait()
server.wait()