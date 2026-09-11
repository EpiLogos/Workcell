#!/usr/bin/env python3
"""Exercise real Workcell binaries in disposable Worlds; never personal Control.
The TCP workloads are test processes, not AIKit/Agent/scheduler implementations.
Run: python3 scripts/caw_native_campaign.py --bin-dir target/debug --output evidence/caw.json
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time

PROTOCOL = "workcell.control/v1"
MAX_FRAME = 16 * 1024 * 1024
TOKEN = "controlled-caw-token-not-a-user-credential"
WORKLOAD = "import socket,sys,time; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',int(sys.argv[1]))); s.listen(); time.sleep(300)"


def check(condition, message):
    if not condition:
        raise RuntimeError(message)


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def receive(sock, length):
    output = bytearray()
    while len(output) < length:
        chunk = sock.recv(length - len(output))
        if not chunk:
            raise RuntimeError("control connection closed before response completed")
        output.extend(chunk)
    return bytes(output)


def request(address, operation, payload=None, *, token=TOKEN, lost_reply=False, expect_ok=True):
    value = {"version": PROTOCOL, "request_id": f"caw:{time.monotonic_ns()}",
             "operation": operation, "payload": payload, "authorization": token}
    encoded = json.dumps(value).encode()
    with socket.create_connection(address, timeout=12) as sock:
        sock.sendall(struct.pack("!I", len(encoded)) + encoded)
        if lost_reply:
            return None
        size, = struct.unpack("!I", receive(sock, 4))
        check(size <= MAX_FRAME, "oversized response")
        response = json.loads(receive(sock, size))
    check(response["request_id"] == value["request_id"], "wrong response correlation")
    check(response["ok"] is expect_ok, f"{operation}: {response.get('error', response)}")
    return response.get("payload") if expect_ok else response["error"]


def empty_tiers():
    return {"required": [], "preferred": [], "optional": []}


def demand():
    value = {name: empty_tiers() for name in ("affordances", "connectivity", "exposure", "outputs", "storage")}
    value.update(demand_ref="demand:caw-native", subjects={
        "agent": "agent:opaque", "session": "session:opaque", "attempt": "attempt:opaque",
        "source": "source:now/opaque", "policy": "policy:opaque/revision-1"},
        workspace=None, project_runtime=None, resources=[], persistence=None,
        isolation_trust=None, retention="release", extensions={})
    value["connectivity"]["required"] = [f"service:{name}-fixture" for name in ("gateway", "session", "scheduler")]
    value["storage"]["required"] = [{"logical_ref": "now:opaque", "access": "writable", "sharing": "shared",
        "minimum_capacity": None, "unit": None, "persistence": "external", "retention": "preserve"}]
    return value


def bindings(world):
    return world["binding_graph"]["bindings"]


def service_pids(world):
    return [int(b["properties"]["pid"]) for b in bindings(world) if b["port"] == "service"]


def stop(process):
    if process.poll() is None:
        process.kill()
    process.wait(timeout=5)


def await_condition(condition, label, timeout=6):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(.025)
    raise RuntimeError(f"timeout: {label}")


def reachable(port_number):
    try:
        with socket.create_connection(("127.0.0.1", port_number), timeout=.15):
            return True
    except OSError:
        return False


def launch(binary, root, reference, processes):
    address = ("127.0.0.1", port())
    log = open(root / "host.log", "ab", buffering=0)
    child = subprocess.Popen([str(binary), "--listen", f"{address[0]}:{address[1]}",
        "--state-root", str(root), "--workcell-ref", reference],
        env={**os.environ, "WORKCELL_CONTROL_TOKEN": TOKEN}, stdin=subprocess.DEVNULL,
        stdout=log, stderr=log)
    log.close()
    processes.append(child)
    def ready():
        check(child.poll() is None, f"control host exited: {(root/'host.log').read_text()}")
        return reachable(address[1])
    await_condition(ready, "native control service ready")
    return child, address


def campaign(binary_dir):
    processes = []
    cases = []
    evidence = {"schema": "workcell.caw-native-campaign/v1", "scope": "controlled-same-host-native-processes",
                "not_proved": ["installed personal World", "actual AIKit gateway/session/recurrence", "physical second host", "private policy adoption", "live VM/container provider"],
                "binaries": {name: hashlib.sha256((binary_dir/name).read_bytes()).hexdigest()
                    for name in ("workcell", "workcell-control-service", "workcell-write-boundary")}}
    with tempfile.TemporaryDirectory(prefix="workcell-caw-native-") as temporary:
        root = Path(temporary)
        now = root / "NOW"
        now.mkdir()
        (now / "pending-return.txt").write_bytes(b"exact pending Return bytes\x00")
        human = root / "human-source.txt"
        human.write_bytes(b"human source is not Workcell property")
        initial = hashlib.sha256(human.read_bytes()).hexdigest()
        state = root / "host-a"
        state.mkdir()
        ports = [port() for _ in range(3)]
        declarations = []
        for name, endpoint in zip(("gateway", "session", "scheduler"), ports):
            declarations.append({"logical_ref": f"service:{name}-fixture", "endpoint": f"tcp://127.0.0.1:{endpoint}",
                "lifetime": "provider-process-scoped", "program": sys.executable,
                "args": ["-S", "-c", WORKLOAD, str(endpoint)], "cwd": str(now),
                "readiness": {"host": "127.0.0.1", "port": endpoint, "timeout_ms": 5000}})
        (state / "services.json").write_text(json.dumps({"schema": "workcell.service-declaration/v1", "services": declarations}))
        storage = {"schema": "workcell.directory-storage/v1", "directories": [{"logical_ref": "now:opaque", "path": str(now)}]}
        (state / "storage.json").write_text(json.dumps(storage))
        try:
            host, address = launch(binary_dir/"workcell-control-service", state, "workcell:caw-a", processes)
            request(address, "status", token="wrong", expect_ok=False)
            slow = socket.create_connection(address)
            slow.sendall(b"\x00\x00")
            started = time.monotonic()
            request(address, "status")
            check(time.monotonic()-started < 5, "lost/trickling client wedged control host")
            slow.close()
            cases.append("authentication-refusal-and-partial-frame-deadline")
            plan = request(address, "plan", demand())
            check(plan["status"] != "unsatisfiable", "controlled demand must be satisfiable")
            with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
                worlds = list(pool.map(lambda _: request(address, "prepare", demand()), range(6)))
            world = worlds[0]
            check(all(w == world for w in worlds), "duplicate preparation created different worlds")
            check(len(bindings(world)) == 4, "storage/connectivity dropped at native boundary")
            check(len(service_pids(world)) == 3, "native service processes were not allocated")
            check(world["subjects"] == demand()["subjects"], "semantic correlations changed")
            cases.append("concurrent-idempotent-native-prepare-with-three-services-and-NOW")
            request(address, "prepare", demand(), lost_reply=True)
            check(request(address, "inspect", {"world_ref": world["world_ref"]}) == world, "lost reply changed allocation")
            check(all(reachable(p) for p in ports), "short-lived clients terminated persistent services")
            changed = demand(); changed["storage"]["required"][0]["logical_ref"] = "different-now"
            request(address, "prepare", changed, expect_ok=False)
            cases.append("lost-client-reconnect-and-changed-demand-refusal")
            stop(host)
            if sys.platform.startswith("linux"):
                await_condition(lambda: not any(reachable(p) for p in ports), "direct managed children stop on host death")
            else:
                # Non-Linux abrupt orphan handling is deliberately not claimed.
                for pid in service_pids(world):
                    try: os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError: pass
            host, address = launch(binary_dir/"workcell-control-service", state, "workcell:caw-a", processes)
            check(request(address, "inspect", {"world_ref": world["world_ref"]}) == world, "restart lost receipt")
            recovered = request(address, "recover", {"world_ref": world["world_ref"]})
            check(recovered["subjects"] == world["subjects"], "recovery changed semantic correlations")
            check(recovered["world_ref"] != world["world_ref"], "new native children reused old material identity")
            check(set(service_pids(world)).isdisjoint(service_pids(recovered)), "recovery did not replace actual children")
            request(address, "release", {"world_ref": world["world_ref"]}, expect_ok=False)
            check(request(address, "recover", {"world_ref": recovered["world_ref"]})["world_ref"] == recovered["world_ref"], "healthy recover spawned duplicates")
            request(address, "release", {"world_ref": recovered["world_ref"]})
            await_condition(lambda: not any(reachable(p) for p in ports), "release stopped actual managed services")
            stop(host)
            host, address = launch(binary_dir/"workcell-control-service", state, "workcell:caw-a", processes)
            request(address, "prepare", demand(), expect_ok=False)
            cases.append("host-death-restart-inspect-recover-supersession-release-tombstone")
            # Substitute the existing target-owned provider, not a simulated reply.
            state_b = root / "host-b"; state_b.mkdir()
            external = []
            for name, endpoint in zip(("gateway", "session", "scheduler"), ports):
                process = subprocess.Popen([sys.executable, "-S", "-c", WORKLOAD, str(endpoint)], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                processes.append(process)
                await_condition(lambda p=endpoint: reachable(p), "external service ready")
                external.append({"logical_ref": f"service:{name}-fixture", "endpoint": f"tcp://127.0.0.1:{endpoint}",
                    "lifetime": "target-owned", "acquisition": "observe-existing", "status": {"program": sys.executable,
                    "args": ["-S", "-c", "import socket,sys; socket.create_connection(('127.0.0.1',int(sys.argv[1])),.5).close()", str(endpoint)]}})
            (state_b/"services.json").write_text(json.dumps({"schema": "workcell.service-declaration/v1", "services": external}))
            (state_b/"storage.json").write_text(json.dumps(storage))
            other, address_b = launch(binary_dir/"workcell-control-service", state_b, "workcell:caw-b", processes)
            substituted = request(address_b, "prepare", demand())
            check(substituted["subjects"] == world["subjects"], "provider substitution changed subjects")
            check(substituted["workcell_ref"] != world["workcell_ref"], "second material host reused Workcell identity")
            check(all(b["properties"].get("lifetime") == "target-owned" for b in bindings(substituted) if b["port"] == "service"), "target provider not used")
            stop(other)
            other, address_b = launch(binary_dir/"workcell-control-service", state_b, "workcell:caw-b", processes)
            request(address_b, "recover", {"world_ref": substituted["world_ref"]})
            request(address_b, "release", {"world_ref": substituted["world_ref"]})
            check(all(reachable(p) for p in ports), "release killed unowned target service")
            cases.append("actual-managed-to-target-owned-provider-substitution-and-reentry")
            check(hashlib.sha256(human.read_bytes()).hexdigest() == initial, "human source changed")
            check((now/"pending-return.txt").read_bytes() == b"exact pending Return bytes\x00", "pending Return lost")
            cases.append("source-and-pending-Return-byte-preservation")
            capability = subprocess.run([str(binary_dir/"workcell-write-boundary"), "capabilities"], check=True, capture_output=True, text=True, timeout=5)
            evidence.update(cases=cases, status="passed", write_boundary_capabilities=json.loads(capability.stdout),
                placement_correlation={"previous_world_ref": world["world_ref"], "recovered_world_ref": recovered["world_ref"], "substituted_world_ref": substituted["world_ref"], "subjects": world["subjects"]})
        finally:
            for process in reversed(processes):
                stop(process)
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, default=Path("target/debug"))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    evidence = campaign(args.bin_dir.resolve())
    encoded = json.dumps(evidence, indent=2)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded+"\n")
    print(encoded)


if __name__ == "__main__":
    main()
