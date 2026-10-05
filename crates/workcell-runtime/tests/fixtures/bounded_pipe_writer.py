"""Actual forked writer for capture lifetime tests; no owner protocol replies."""
import errno
import json
import os
import pathlib
import sys
import time

root = pathlib.Path(os.environ.get("WORKCELL_PIPE_FIXTURE_ROOT") or sys.argv[1]).resolve(strict=True)
release = root / "release"


def publish(name, value):
    stage = root / ("." + name + ".writing")
    with stage.open("x") as out:
        json.dump(value, out)
        out.flush()
        os.fsync(out.fileno())
    os.link(stage, root / name)
    stage.unlink()


child = os.fork()
if child == 0:
    if (os.environ.get("WORKCELL_PIPE_ESCAPE") or (sys.argv[2] if len(sys.argv) > 2 else "0")) == "1":
        os.setsid()
    os.write(1, b"out-prefix\n")
    os.write(2, b"err-prefix\n")
    publish("writer.json", {"pid": os.getpid(), "ppid": os.getppid(), "pgid": os.getpgrp(),
                            "uid": os.getuid()})
    deadline = time.monotonic() + 8
    while not release.exists() and time.monotonic() < deadline:
        time.sleep(.005)
    results = {}
    for fd in (1, 2):
        try:
            results[str(fd)] = {"written": os.write(fd, b"late-bytes\n")}
        except OSError as error:
            results[str(fd)] = {"errno": error.errno, "epipe": error.errno == errno.EPIPE}
    for fd in (1, 2):
        os.close(fd)
    publish("writer-result.json", {"pid": os.getpid(), "released": release.exists(), "writes": results})
    os._exit(0)
deadline = time.monotonic() + 2
while not (root / "writer.json").exists():
    if time.monotonic() >= deadline:
        os._exit(3)
    time.sleep(.005)
if os.environ.get("WORKCELL_PIPE_TIMEOUT_GROUP") == "1":
    deadline = time.monotonic() + 8
    while not release.exists() and time.monotonic() < deadline:
        time.sleep(.005)
    os._exit(4)
os._exit(0)
