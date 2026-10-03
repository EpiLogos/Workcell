"""Real stdlib TCP target; actual OS birth, admission and retirement facts.
This exercises Workcell's production provider, without representing a model.
"""
import base64
import fcntl
import hashlib
import http.server
import json
import os
import pathlib
import secrets
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(sys.argv[2]).resolve()
PORT = int(sys.argv[3])
ENDPOINT = "http://127.0.0.1:" + str(PORT)
SCHEMA = "workcell.external-target-instance/v1"
OBSERVATION_SCHEMA = "workcell.external-target-observation/v1"
STOP_SCHEMA = "workcell.external-target-stop/v1"
NATIVE_GENERATION = None


class NativeProcessObservationFailed(RuntimeError):
    def __init__(self, exit_status, stdout, stderr):
        super().__init__("actual native process observation failed; retained private diagnostic")
        self.native_observation = {"exit_status": exit_status, "stdout_base64": base64.b64encode(stdout).decode("ascii"),
                                   "stderr_base64": base64.b64encode(stderr).decode("ascii")}


class TargetAbsent(Exception):
    pass


class NotReady(Exception):
    pass


def write_once(path, value):
    with path.open("x", encoding="utf-8") as out:
        json.dump(value, out, sort_keys=True)
        out.flush()
        os.fsync(out.fileno())


def bytes_of(path):
    with path.open("rb") as source:
        data = source.read(16_385)
    if len(data) > 16_384:
        raise ValueError("native retained record exceeded bound")
    return data


def read(path):
    return json.loads(bytes_of(path))


def digest(path):
    return hashlib.sha256(bytes_of(path)).hexdigest()


def identity(pid, include_state=False):
    # Deadline and bounded reads; only this exact owned ps child is killed.
    with tempfile.TemporaryFile(dir=ROOT) as output, tempfile.TemporaryFile(dir=ROOT) as error:
        process = subprocess.Popen(
            ["/bin/ps", "-p", str(pid), "-o", "pid=,uid=,lstart=,stat="],
            stdout=output, stderr=error, stdin=subprocess.DEVNULL,
            env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"})
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=2)
            raise RuntimeError("actual native process observation timed out")
        output.seek(0)
        data = output.read(4097)
        error.seek(0)
        private_error = error.read(4097)
    if len(data) > 4096 or len(private_error) > 4096:
        raise RuntimeError("actual native process observation exceeded output bound")
    fields = data.decode("utf-8", errors="strict").split()
    if process.returncode == 1 and data == b"" and private_error == b"":
        return None
    if process.returncode != 0 or len(fields) != 8 or private_error:
        raise NativeProcessObservationFailed(process.returncode, data, private_error)
    birth = {"pid": int(fields[0]), "uid": int(fields[1]), "start": " ".join(fields[2:7])}
    # A zombie retains this exact native identity until its actual owner reaps
    # it. Process state never weakens the immutable birth comparison.
    return {**birth, "state": fields[7]} if include_state else birth


def directory(generation):
    if len(generation) != 32 or any(c not in "0123456789abcdef" for c in generation):
        raise ValueError("invalid generation")
    return ROOT / generation


def basis_path(generation):
    return directory(generation) / "basis.json"


def retained_basis():
    global NATIVE_GENERATION
    path = pathlib.Path(os.environ["WORKCELL_TARGET_BASIS_PATH"]).resolve(strict=True)
    if path.parent.parent != ROOT or path.name != "basis.json":
        raise ValueError("basis is outside original owned generation")
    if digest(path) != os.environ["WORKCELL_TARGET_BASIS_SHA256"]:
        raise ValueError("original basis hash differs")
    basis = read(path)
    if basis["generation"] != os.environ["WORKCELL_TARGET_GENERATION"]:
        raise ValueError("original generation differs")
    NATIVE_GENERATION = basis["generation"]
    return path, basis


def retirement(path, basis, acknowledged=False):
    if identity(basis["server"]["pid"]) == basis["server"]:
        raise ValueError("original server is still present")
    retired = path.parent / "retired.json"
    if retired.exists():
        result = read(retired)
        if result["generation"] != basis["generation"] or result["basis_sha256"] != digest(path):
            raise ValueError("retirement belongs to different native instance")
        return retired, result
    result = {"schema": STOP_SCHEMA, "generation": basis["generation"],
              "basis_sha256": digest(path), "server": basis["server"], "retired": True,
              "acknowledged": acknowledged, "stop_effect": "stopped" if acknowledged else "unknown",
              "native_quiescence_verified": acknowledged}
    write_once(retired, result)
    return retired, result


def stop_envelope(path, basis, repeated=False):
    retired, result = retirement(path, basis)
    effect = "already-retired" if repeated and result["acknowledged"] else result["stop_effect"]
    return {**result, "stop_effect": effect, "evidence_path": str(retired),
            "evidence_sha256": digest(retired)}


class NativeNoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, file, code, message, headers, newurl):
        # Returning no request preserves the actual HTTP refusal; this owner
        # cannot transfer generation/config to any redirected endpoint.
        return None


def native_request(basis, operation):
    if basis["endpoint"] != ENDPOINT:
        raise ValueError("native fixture basis does not name its owned loopback endpoint")
    request = urllib.request.Request(basis["endpoint"] + "/" + operation,
        headers={"X-Generation": basis["generation"], "X-Config": basis["config_sha256"]})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NativeNoRedirect())
    with opener.open(request, timeout=2) as response:
        data = response.read(16_385)
    if len(data) > 16_384:
        raise ValueError("native HTTP output exceeded bound")
    result = json.loads(data)
    if (result["generation"] != basis["generation"] or result["server"] != basis["server"]
            or result["config_sha256"] != basis["config_sha256"] or result["endpoint"] != basis["endpoint"]):
        raise ValueError("same endpoint serves different native instance")
    return result


def known_unbound_basis():
    global NATIVE_GENERATION
    current = ROOT / "current"
    if current.exists():
        generation = current.read_text()
        NATIVE_GENERATION = generation
        path = basis_path(generation)
        if not path.exists():
            launcher = read(directory(generation) / "launcher.json")
            if identity(launcher["pid"]) == launcher:
                raise NotReady("actual launcher has not published basis")
            raise ValueError("retained intent has no qualified basis")
        basis = read(path)
        if identity(basis["server"]["pid"]) == basis["server"]:
            return path, basis
        retirement(path, basis)
    # Failed HTTP never proves absence. Native lifetime lock and every
    # retained exact launcher/server identity must actually be absent.
    with (ROOT / "instance.lock").open("a+b") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise NotReady("actual target lifetime lock is owned")
        for entry in ROOT.iterdir():
            if not entry.is_dir() or not (entry / "intent.json").exists():
                continue
            if (entry / "basis.json").exists():
                native = read(entry / "basis.json")["server"]
            elif (entry / "launcher.json").exists():
                native = read(entry / "launcher.json")
            else:
                raise ValueError("unqualified retained start intent")
            if identity(native["pid"]) == native:
                raise NotReady("a retained target identity is alive")
        fcntl.flock(lock, fcntl.LOCK_UN)
    raise TargetAbsent("retained identities absent and native lifetime lock free")


def serve(generation, config):
    with (ROOT / "instance.lock").open("a+b") as lifetime:
        fcntl.flock(lifetime, fcntl.LOCK_EX | fcntl.LOCK_NB)
        basis = {"schema": SCHEMA, "generation": generation, "endpoint": ENDPOINT,
                 "config_sha256": config, "server": identity(os.getpid()),
                 "private_native_key": secrets.token_hex(32)}
        staged_basis = directory(generation) / ".basis.writing"
        # Capture must see either no basis or the complete original basis.
        # Exclusive staging and no-overwrite linking preserve prior evidence.
        write_once(staged_basis, basis)
        barrier = os.environ.get("TEST_BASIS_BARRIER")
        if barrier:
            release = ROOT / barrier
            if release.parent != ROOT or release.name != "basis-publication-release":
                raise ValueError("publication barrier is outside owned test target")
            stage = directory(generation) / ".publication-held.writing"
            write_once(stage, {"generation": generation, "server": identity(os.getpid())})
            os.link(stage, directory(generation) / "publication-held.json")
            stage.unlink()
            while not release.exists():
                time.sleep(.01)
        time.sleep(float(os.environ.get("TEST_BASIS_DELAY", "0")))
        os.link(staged_basis, basis_path(generation))
        staged_basis.unlink()
        gate = threading.Lock()
        pending = 0
        accepting = True
        refuse_stop = os.environ.get("TEST_REFUSE_STOP") == "1"

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def respond(self, status, body):
                data = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                nonlocal pending, accepting, refuse_stop
                operation = self.path[1:]
                if operation == "refused_read":
                    self.respond(409, {"error": "actual native read refusal"})
                    return
                if self.headers.get("X-Generation") != generation or self.headers.get("X-Config") != config:
                    self.respond(409, {"error": "native instance mismatch"})
                    return
                if operation == "hold":
                    with gate:
                        if not accepting:
                            self.respond(409, {"error": "admission closed"})
                            return
                        pending += 1
                    try:
                        time.sleep(1.5)
                    finally:
                        with gate:
                            pending -= 1
                if operation == "quiesce":
                    with gate:
                        accepting = False
                if operation == "resume":
                    with gate:
                        accepting = True
                if operation == "allow_stop":
                    with gate:
                        refuse_stop = False
                if operation == "stop":
                    with gate:
                        if pending or refuse_stop:
                            self.respond(409, {"error": "actual active dependency or explicit refusal", "pending": pending})
                            return
                        accepting = False
                    if os.environ.get("TEST_LOSE_STOP_ACK") == "1":
                        self.close_connection = True
                    else:
                        self.respond(200, basis)
                    threading.Thread(target=server.shutdown, daemon=True).start()
                    return
                if operation == "ready" and os.environ.get("TEST_NEVER_READY") == "1":
                    self.respond(503, {"error": "actual server readiness refused"})
                    return
                self.respond(200, {**basis, "pending": pending, "accepting": accepting,
                                   "native_healthy": accepting and os.environ.get("TEST_NEVER_READY") != "1"})

        server = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
        server.daemon_threads = False
        try:
            server.serve_forever(poll_interval=.05)
        finally:
            server.server_close()


def zombie_oracle():
    # This command is the actual parent of this bounded child, so wait() is
    # its own native reap. It never adopts/reaps a detached target server.
    child = subprocess.Popen([sys.executable, "-S", "-c", "import time; time.sleep(.05)"],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.monotonic() + 4
    try:
        while True:
            observed = identity(child.pid, include_state=True)
            if observed is not None and observed["state"].startswith("Z"):
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("actual owned child zombie observation unqualified")
            time.sleep(.01)
        birth = {key: observed[key] for key in ("pid", "uid", "start")}
        if identity(child.pid) != birth:
            raise RuntimeError("zombie identity was incorrectly treated as absent")
        evidence = ROOT / "zombie-observed.json"
        write_once(evidence, observed)
        try:
            retirement(evidence, {"generation": "actual-owned-child-oracle", "server": birth})
        except ValueError as error:
            if str(error) != "original server is still present":
                raise
        else:
            raise RuntimeError("native retirement accepted an unreaped original zombie")
        # No poll() or wait() occurred before the actual zombie evidence.
        exit_code = child.wait(timeout=2)
        disappeared = identity(child.pid) is None
        if exit_code != 0 or not disappeared:
            raise RuntimeError("actual owner reap did not prove original physical disappearance")
        print(json.dumps({"observed": observed, "same_process_before_reap": True,
                          "retirement_refused_before_reap": True, "owner_wait_exit": exit_code,
                          "physically_absent_after_owner_reap": disappeared}))
    finally:
        if child.returncode is None:
            # Exact owned child only; this cannot signal or reap target service.
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=2)


def emit_observation(path, basis, actual):
    if type(actual.get("native_healthy")) is not bool:
        raise ValueError("actual native read has no semantic health witness")
    if type(actual.get("pending")) is not int or actual["pending"] < 0:
        raise ValueError("actual native read has no admission count witness")
    print(json.dumps({"schema": OBSERVATION_SCHEMA, "basis_path": str(path),
                      "basis_sha256": digest(path), "generation": actual["generation"],
                      "config_sha256": actual["config_sha256"], "endpoint": actual["endpoint"],
                      "server": actual["server"], "native_healthy": actual["native_healthy"],
                      "pending": actual["pending"]}))


def main(operation):
    global NATIVE_GENERATION
    if operation == "zombie_oracle":
        zombie_oracle()
        return
    if operation == "serve":
        serve(sys.argv[4], sys.argv[5])
        return
    if operation == "start":
        if os.environ.get("WORKCELL_TARGET_INSTANCE_PROTOCOL") != SCHEMA:
            raise ValueError("native protocol absent before any start effect")
        generation = secrets.token_hex(16)
        NATIVE_GENERATION = generation
        target = directory(generation)
        target.mkdir()
        intent = target / "intent.json"
        write_once(intent, {"generation": generation, "endpoint": ENDPOINT})
        with (target / "output.log").open("xb") as output:
            process = subprocess.Popen([sys.executable, "-S", __file__, "serve", str(ROOT), str(PORT), generation,
                hashlib.sha256(os.environ.get("TEST_CONFIG", "config-a").encode()).hexdigest()],
                stdout=output, stderr=output, stdin=subprocess.DEVNULL, start_new_session=True)
        observed = identity(process.pid)
        if observed is None:
            raise ValueError("actual launcher exited before retained birth")
        write_once(target / "launcher.json", observed)
        (ROOT / "current").write_text(generation)
        if os.environ.get("TEST_BASIS_BARRIER"):
            held = target / "publication-held.json"
            deadline = time.monotonic() + 2
            while not held.exists():
                if identity(process.pid) != observed:
                    raise ValueError("actual launcher exited before publication barrier")
                if time.monotonic() >= deadline:
                    raise ValueError("actual launcher did not reach publication barrier; start intent retained")
                time.sleep(.01)
            witness = read(held)
            if witness["generation"] != generation or witness["server"] != observed:
                raise ValueError("publication barrier names a different native birth")
        print(json.dumps({"schema": "workcell.external-target-start/v1", "generation": generation,
            "started": True, "intent_path": str(intent), "intent_sha256": digest(intent)}), flush=True)
        # The native start and public envelope are genuine. Only the real
        # command's later exit is adverse; no provider reply is fabricated.
        if os.environ.get("TEST_START_EXIT_AFTER_ENVELOPE"):
            sys.exit(23)
        return
    if operation == "capture":
        generation = os.environ.get("WORKCELL_TARGET_GENERATION")
        if generation:
            NATIVE_GENERATION = generation
            path = basis_path(generation)
            if not path.exists():
                launcher = read(directory(generation) / "launcher.json")
                if identity(launcher["pid"]) == launcher:
                    raise NotReady("original owned launcher has not published basis")
                raise ValueError("original intent has no qualified live basis")
            basis = read(path)
            if identity(basis["server"]["pid"]) != basis["server"]:
                retirement(path, basis)
                raise TargetAbsent("original process is physically absent")
        elif "WORKCELL_TARGET_BASIS_PATH" in os.environ:
            path, basis = retained_basis()
            if identity(basis["server"]["pid"]) != basis["server"]:
                retirement(path, basis)
                raise TargetAbsent("original process is physically absent")
        else:
            path, basis = known_unbound_basis()
        print(json.dumps({**basis, "basis_path": str(path), "basis_sha256": digest(path)}))
        return
    if operation == "current_status":
        # Actual observational CLI route intentionally reads its latest
        # native generation, never performs mutation. Workcell must reject
        # this genuine successor witness beneath an original receipt.
        path, basis = known_unbound_basis()
        emit_observation(path, basis, native_request(basis, "status"))
        return
    if "WORKCELL_TARGET_BASIS_PATH" in os.environ:
        path, basis = retained_basis()
        observed = identity(basis["server"]["pid"], include_state=True)
        same_process = observed is not None and {key: observed[key] for key in ("pid", "uid", "start")} == basis["server"]
        if operation == "identity":
            print(json.dumps({"generation": basis["generation"], "server": basis["server"],
                              "same_process": same_process, "state": None if observed is None else observed["state"]}))
            return
        if not same_process:
            if operation == "stop":
                print(json.dumps(stop_envelope(path, basis, repeated=True)))
                return
            retirement(path, basis)
            raise TargetAbsent("original process is physically absent")
    else:
        path, basis = known_unbound_basis()
    if operation == "stop":
        acknowledged = True
        try:
            native_request(basis, operation)
        except urllib.error.HTTPError:
            raise
        except (urllib.error.URLError, OSError, TimeoutError):
            acknowledged = False
        deadline = time.monotonic() + 4
        while identity(basis["server"]["pid"]) == basis["server"]:
            if time.monotonic() >= deadline:
                raise ValueError("original process retirement unconfirmed")
            time.sleep(.02)
        retirement(path, basis, acknowledged)
        print(json.dumps(stop_envelope(path, basis)), flush=True)
        # Actual native shutdown/absence and envelope precede this nonzero
        # command exit. They cannot author an acknowledged Workcell cleanup.
        if os.environ.get("TEST_STOP_EXIT_AFTER_ENVELOPE"):
            sys.exit(24)
        return
    actual = native_request(basis, operation)
    if operation in ("status", "ready") and os.environ.get("WORKCELL_TARGET_INSTANCE_PROTOCOL") == SCHEMA:
        emit_observation(path, basis, actual)
    else:
        print(json.dumps(actual))


def emit_native_failure(error):
    kind = "operation-failed"
    if isinstance(error, TargetAbsent):
        kind = "target-absent"
    elif isinstance(error, NotReady):
        kind = "not-ready"
    elif isinstance(error, urllib.error.HTTPError):
        kind = "not-ready" if error.code == 503 else "native-http-refusal"
    public = {"schema": "workcell.external-target-error/v1", "kind": kind, "native_type": type(error).__name__}
    if isinstance(error, urllib.error.HTTPError):
        public["http_status"] = error.code
    if NATIVE_GENERATION is not None:
        public["generation"] = NATIVE_GENERATION
        target = directory(NATIVE_GENERATION)
        evidence = None
        if isinstance(error, TargetAbsent) and (target / "retired.json").exists():
            evidence = target / "retired.json"
        elif target.is_dir():
            evidence = target / ("observation-" + secrets.token_hex(16) + ".json")
            private = {"native_type": type(error).__name__, "detail": str(error)}
            if isinstance(error, NativeProcessObservationFailed):
                private["actual_native_process_observation"] = error.native_observation
            if isinstance(error, urllib.error.HTTPError):
                private["http_status"] = error.code
                private["native_body"] = error.read(16_384).decode("utf-8", errors="replace")
            write_once(evidence, private)
        if evidence is not None:
            public["evidence_path"] = str(evidence)
            public["evidence_sha256"] = digest(evidence)
    if NATIVE_GENERATION is None and isinstance(error, NativeProcessObservationFailed):
        # Failed unbound observation also retains its actual private cause;
        # the public envelope carries only this owned evidence reference/hash.
        evidence = ROOT / ("observation-" + secrets.token_hex(16) + ".json")
        write_once(evidence, {"native_type": type(error).__name__,
                              "actual_native_process_observation": error.native_observation})
        public["evidence_path"] = str(evidence)
        public["evidence_sha256"] = digest(evidence)
    print(json.dumps(public), file=sys.stderr)


try:
    main(sys.argv[1])
except Exception as error:
    emit_native_failure(error)
    sys.exit(1)
