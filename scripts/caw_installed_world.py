#!/usr/bin/env python3
"""Read-only installed census and explicit two-placement native lifecycle proving.
Nothing installs software, migrates Control/NOW, alters permissions or edits policy.
A census is not acceptance. `exercise` needs an authorised test-World packet.
Evidence is private by default (new 0600 file); no automatic upload or overwrite.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time
from urllib.parse import urlparse

from caw_native_campaign import request, check


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024*1024), b""):
            h.update(chunk)
    return h.hexdigest()


def save(path, value):
    data = (json.dumps(value, indent=2)+"\n").encode()
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def census(args):
    binary = args.binary.resolve(strict=True)
    root = args.state_root.resolve(strict=True)
    check(root.is_dir(), "state root must already exist")
    version = subprocess.run([str(binary), "--version"], check=True, capture_output=True, text=True, timeout=5).stdout.strip()
    source = None
    if args.repo:
        repo = args.repo.resolve(strict=True)
        source = {"commit": subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True, timeout=5).strip(),
                  "dirty": bool(subprocess.check_output(["git", "-C", str(repo), "status", "--porcelain"], timeout=5))}
    identity = Path("/etc/machine-id")
    identity_bytes = identity.read_bytes() if identity.is_file() else platform.node().encode()
    declarations = {}
    for name in ("services.json", "storage.json"):
        path = root/name
        if path.is_file():
            declarations[name] = {"sha256": digest(path), "bytes": path.stat().st_size}
    return {"schema": "workcell.installed-census/v1", "standing": "read-only-observation-not-acceptance",
            "observed_at_unix_ms": time.time_ns()//1_000_000,
            "host_identity_sha256": hashlib.sha256(identity_bytes).hexdigest(),
            "os": platform.system(), "kernel": platform.release(), "architecture": platform.machine(),
            "binary": {"path": str(binary), "sha256": digest(binary), "version": version},
            "source": source, "state_root": str(root), "declarations": declarations,
            "limits": ["local census does not authenticate a remote endpoint", "no source or secret file contents collected",
                       "no installation, migration, permission or governance change"]}


def address(endpoint):
    parsed = urlparse(endpoint)
    check(parsed.scheme == "tcp" and parsed.hostname and parsed.port and not parsed.username and not parsed.password,
          "endpoint must be tcp://HOST:PORT, credentials only in the named environment variable")
    check(parsed.path in ("", "/") and not parsed.query and not parsed.fragment, "invalid control endpoint")
    return parsed.hostname, parsed.port


def exercise(args, evidence, checkpoint=lambda: None):
    check(args.manifest.stat().st_size <= 1024 * 1024, "campaign packet exceeds 1 MiB")
    packet = json.loads(args.manifest.read_text())
    check(args.execute_authorized and packet.get("authorized_material_effects") is True and packet.get("test_world") is True,
          "exercise requires --execute-authorized and an explicit authorised test_world packet")
    check(packet.get("schema") == "workcell.installed-campaign/v1", "unsupported campaign schema")
    check(isinstance(packet.get("release_after_test"), bool), "explicit release_after_test choice required")
    sites = packet["placements"]
    check(len(sites) == 2, "exactly two placements are required")
    check(sites[0]["workcell_ref"] != sites[1]["workcell_ref"], "placements reuse Workcell identity")
    check(sites[0]["endpoint"] != sites[1]["endpoint"], "placements reuse endpoint")
    # Require real independently collected census files, not a second directory
    # on this host. These are evidence inputs, not remote-host attestation.
    censuses = [json.loads(Path(site["census_file"]).read_text()) for site in sites]
    check(all(c.get("schema") == "workcell.installed-census/v1" for c in censuses), "missing installed census")
    check(censuses[0]["host_identity_sha256"] != censuses[1]["host_identity_sha256"], "same-host simulation is not second-placement proof")
    evidence["census_sha256"] = [digest(Path(site["census_file"])) for site in sites]
    evidence["host_evidence_standing"] = "supplied per-host census; endpoint-to-host binding still requires independent local verification"
    source_demand = packet["demand"]
    check(source_demand.get("subjects"), "explicit semantic correlations required")
    check(source_demand.get("storage", {}).get("required") and source_demand.get("connectivity", {}).get("required"),
          "installed hosting campaign requires explicit service and NOW storage requirements")
    clients = []
    for site in sites:
        restart = site.get("authorized_restart_argv")
        check(isinstance(restart, list) and 0 < len(restart) <= 128 and all(isinstance(a, str) and a for a in restart),
              "explicit bounded native restart argv required before material effects; no inferred supervisor")
        token = os.environ.get(site["token_env"])
        check(bool(token), "named control token environment variable is missing")
        target = address(site["endpoint"])
        call = lambda op, body=None, a=target, t=token: request(a, op, body, token=t)
        discovered = call("discover")
        check(discovered["workcell_ref"] == site["workcell_ref"], "observed Workcell identity does not match authorised placement")
        check(call("plan", source_demand)["status"] == "satisfiable", "placement cannot satisfy the exact demand without downgrade")
        clients.append(call)
    evidence["placements"] = []
    # Effects begin only after both preflights pass. Failure leaves exact
    # allocation/observation records, never a fabricated automatic rollback.
    for site, call in zip(sites, clients):
        entry = {"workcell_ref": site["workcell_ref"], "status": "preparing"}
        evidence["placements"].append(entry)
        checkpoint()
        world = call("prepare", source_demand)
        entry.update(world=world, status="prepared")
        checkpoint()
        check(world["subjects"] == source_demand["subjects"], "semantic identity changed during placement")
        check(call("inspect", {"world_ref": world["world_ref"]}) == world, "public receipt readback changed")
        observation = call("observe", {"world_ref": world["world_ref"]})
        entry["before_restart"] = observation
        check(observation["observations"] and all(o["state"] == "healthy" for o in observation["observations"]), "material world is not healthy")
        restart = site.get("authorized_restart_argv")
        check(isinstance(restart, list) and restart and all(isinstance(a, str) for a in restart), "explicit native restart argv required; no inferred supervisor")
        entry["restart_command_sha256"] = hashlib.sha256(json.dumps(restart).encode()).hexdigest()
        entry["status"] = "restarting"
        checkpoint()
        subprocess.run(restart, check=True, timeout=30, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        entry["status"] = "reconnecting"
        checkpoint()
        deadline = time.monotonic()+30
        while True:
            try:
                check(call("discover")["workcell_ref"] == site["workcell_ref"], "reconnected to wrong Workcell")
                break
            except (OSError, RuntimeError):
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.25)
        entry["status"] = "recovering"
        checkpoint()
        recovered = call("recover", {"world_ref": world["world_ref"]})
        check(recovered["subjects"] == source_demand["subjects"], "recovery changed semantic correlations")
        entry["recovered"] = recovered
        checkpoint()
        entry["after_restart"] = call("observe", {"world_ref": recovered["world_ref"]})
        check(entry["after_restart"]["observations"] and all(o["state"] == "healthy" for o in entry["after_restart"]["observations"]), "recovered material is unhealthy")
        if packet.get("release_after_test") is True:
            entry["status"] = "releasing"
            checkpoint()
            entry["release"] = call("release", {"world_ref": recovered["world_ref"]})
            entry["status"] = "released-as-requested"
        else:
            entry["status"] = "retained-explicitly; release-proof-pending"
        checkpoint()
    check(evidence["placements"][0]["world"]["world_ref"] != evidence["placements"][1]["world"]["world_ref"], "second placement reused material world identity")
    evidence["status"] = "material-campaign-passed-not-whole-feature-acceptance"
    evidence["still_requires"] = ["independent endpoint/host/provider verification", "source/data migration and retained bytes under the authorised local migration plan",
                                  "actual AIKit gateway/session/scheduler and Factory Return", "policy adoption and required installed confinement", "human acceptance"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    read = commands.add_parser("census")
    read.add_argument("--binary", type=Path, required=True)
    read.add_argument("--state-root", type=Path, required=True)
    read.add_argument("--repo", type=Path)
    read.add_argument("--output", type=Path, required=True)
    test = commands.add_parser("exercise")
    test.add_argument("--manifest", type=Path, required=True)
    test.add_argument("--execute-authorized", action="store_true")
    test.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.operation == "census":
        save(args.output, census(args))
        print("Read-only census recorded. No installed state changed.")
        return
    record_exercise(args)
    print("Material campaign recorded; independent and whole-operation proof remains separate.")


def record_exercise(args):
    """Reserve evidence before effects and fsync append-only checkpoints.

    On a forced harness death, the sidecar records the last known operation and
    receipts; the reserved final file is not a success claim. No file is reused.
    """
    evidence = {"schema": "workcell.installed-campaign-evidence/v1", "status": "not-started"}
    descriptor = os.open(args.output, os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w+") as final:
        final.write(json.dumps(evidence)+"\n")
        final.flush()
        os.fsync(final.fileno())
        journal_descriptor = os.open(str(args.output)+".journal.jsonl", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(journal_descriptor, "w") as journal:
            def checkpoint():
                journal.write(json.dumps(evidence)+"\n")
                journal.flush()
                os.fsync(journal.fileno())
            checkpoint()
            try:
                exercise(args, evidence, checkpoint)
            except BaseException as error:
                evidence.update(status="failed-or-interrupted; effects-require-inspection",
                                error_type=type(error).__name__, error=str(error))
                raise
            finally:
                checkpoint()
                final.seek(0)
                final.write(json.dumps(evidence, indent=2)+"\n")
                final.truncate()
                final.flush()
                os.fsync(final.fileno())



if __name__ == "__main__":
    main()
