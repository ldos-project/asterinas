import os
import subprocess
import sys
import time

from pathlib import Path
from threading import Thread
from typing import Literal

from tqdm import tqdm

INIT_TIMEOUT = 5


class VMRedisServer:
    proc: subprocess.Popen
    log: list[str]
    start: float

    def __init__(self, hugepaged_enabled: bool = True):
        hpd_enable_flag = "true" if hugepaged_enabled else "false"

        self.proc = subprocess.Popen(
            [
                "make",
                "run",
                "ENABLE_KVM=1",
                "INITARGS=/benchmark/redis/ycsb/run.sh",
                f"KCMDARGS=vm.hugepaged_enabled={hpd_enable_flag}",
                "MEM=32G",
                "NETDEV=tap",
                "RELEASE=1",
                "RELEASE_LTO=1",
                "SMP=4",
                "VHOST=on",
            ],
            stdout=subprocess.PIPE,
        )
        self.log = []
        self.start = time.time()

    def terminate(self):
        p = self.proc
        p.kill()
        p.wait(timeout=2)
        p.terminate()

        print("KILL?")
        output = subprocess.check_output(["ps", "aux"])
        output = output.decode().split("\n")[1:]
        for line in output:
            if "qemu" in line:
                os.system(f"kill -9 {line.split()[1]}")

        subprocess.check_call(["stty", "sane"])

    def initialize(self):
        p = self.proc
        assert p.stdout
        initialized = False
        while True:
            line = p.stdout.readline().decode()
            print(line, end="")
            self.log.append(line)
            if "Ready to accept connections" not in line:
                now = time.time()
                if (now - self.start) > INIT_TIMEOUT:
                    break
                continue
            initialized = True
            break

        if not initialized:
            self.terminate()
            return False
        return True


# JAVA_HOME = "./test/benchmark/jre/jre1.8.0_471/"
# YCSB_PATH = "./ycsb-0.17.0/bin/ycsb.sh"
YCSB_PATH = "./YCSB/bin/ycsb.sh"
YCSB_WORKLOAD_PATH = "./ycsb-0.17.0/workloads/workload-custom"


class YCSBInvocation:
    phase: str
    params: list[str]
    logs: list[str]

    def __init__(self, phase: Literal["load", "run"], params: list[str] = []):
        self.phase = phase
        self.params = params
        self.logs = []

    def run(self, extra_params: list[str] = []):
        environment = os.environ.copy()
        # environment["JAVA_HOME"] = JAVA_HOME
        self.logs.append(
            subprocess.check_output(
                [
                    YCSB_PATH,
                    self.phase,
                    "redis",
                    "-P",
                    YCSB_WORKLOAD_PATH,
                    "-p",
                    "redis.host=10.0.2.15",
                ]
                + self.params
                + extra_params,
                env=environment,
                stderr=subprocess.DEVNULL,
            ).decode()
        )
        print(self.logs[-1], end="")


def benchmark():
    # Make loads artificially slower for easier stats collection
    yload = YCSBInvocation("load", ["-threads", "1", "-target", "1000"])
    yrun = YCSBInvocation("run")

    # 0, 64, 128, ...=512
    for offset in tqdm(range(0, 513, 64)):
        time.sleep(15)
        yload.run(["-p", f"insertstart={offset}"])
        time.sleep(15)
        yrun.run()

    load_logs = yload.logs
    run_logs = yrun.logs
    return load_logs, run_logs


# Write logs to file
def logger(f: Path, server: VMRedisServer):
    assert server.proc.stdout
    with f.open("w") as fh:
        for line in server.log:
            fh.write(line)

        while True:
            line = server.proc.stdout.readline().decode()
            fh.write(line)
            if not line.startswith("now="):
                print(line, end="")


if __name__ == "__main__":
    hugepaged_enabled = "--hpde" in sys.argv
    while True:
        server = VMRedisServer(hugepaged_enabled=hugepaged_enabled)
        if server.initialize():
            print("SERVER INTIALIZED!!!!")
            break

    dir_ = "ycsb_logs"
    if hugepaged_enabled:
        dir_ += "_hugepage"
    os.system(f"mkdir -p {dir_}")

    t = Thread(target=logger, args=(Path(f"{dir_}/server.log"), server))
    t.start()

    load_logs, run_logs = benchmark()

    print("Saving client logs")
    with open(f"{dir_}/load.log", "w") as f:
        for line in load_logs:
            f.write(line)

    with open(f"{dir_}/run.log", "w") as f:
        for line in run_logs:
            f.write(line)

    print("shutting down server")
    server.terminate()
    t.join(timeout=5)
    exit(1)
