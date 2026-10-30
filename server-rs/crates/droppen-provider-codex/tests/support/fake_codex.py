#!/usr/bin/env python3
"""A bounded, local App Server fixture; never talks to a real model."""
import json
import os
from pathlib import Path
import sys
import threading
import time

cwd = Path.cwd()
test_root = next((p for p in [cwd, *cwd.parents] if (p / ".rh-test-root").exists()), None)
if test_root is None:
    sys.exit(97)
(cwd / "codex-argv.bin").write_bytes(b"\0".join(a.encode() for a in sys.argv[1:]) + b"\0")
(cwd / "codex-env.txt").write_text("\n".join(f"{k}={v}" for k, v in os.environ.items()))
with (cwd / "codex-calls.txt").open("a") as calls:
    calls.write("spawn\n")
def archive_thread(thread_id):
    destination = test_root / ".codex/archived_sessions"
    destination.mkdir(parents=True, exist_ok=True)
    for path in (test_root / ".codex/sessions").rglob(f"*-{thread_id}.jsonl"):
        path.rename(destination / path.name)


if len(sys.argv) == 3 and sys.argv[1] == "archive":
    archive_thread(sys.argv[2])
    sys.exit(0)
# An account's model provider follows the subcommand as `-c` overrides.
if sys.argv[1:2] != ["app-server"]:
    sys.exit(0)
if (cwd / "codex-exit.txt").exists():
    sys.exit(int((cwd / "codex-exit.txt").read_text()))

thread = "0199c1a1-2b3c-4d5e-8f60-112233445566"
plan = cwd / "codex-stdout.txt"
if plan.exists():
    for line in plan.read_text().splitlines():
        event = json.loads(line)
        thread = event.get("thread_id", thread)
active = None
sequence = 0
timer = None
lock = threading.RLock()


def emit(value):
    print(json.dumps(value), flush=True)


def complete(turn, status="completed"):
    global active
    with lock:
        if active != turn:
            return
        active = None
        emit({"method": "turn/completed", "params": {
            "threadId": thread, "turn": {"id": turn, "status": status}}})


try:
    for raw in sys.stdin:
        request = json.loads(raw)
        with lock:
            with (cwd / "codex-stdin.jsonl").open("a") as log:
                log.write(raw)
            method = request.get("method")
            params = request.get("params", {})
            if "id" not in request:
                continue
            result = {}
            # A start held at its first answer for as long as `init-hold` is
            # there: what a slow app server looks like to the server.
            if method == "initialize":
                while (cwd / "init-hold").exists():
                    time.sleep(0.01)
            if method in ("thread/start", "thread/resume"):
                thread = params.get("threadId", thread)
                if method == "thread/resume" and (cwd / "resume-thread.txt").exists():
                    thread = (cwd / "resume-thread.txt").read_text().strip()
                result ={"thread": {"id": thread, "turns": []}, "model": "test-model",
                          "reasoningEffort": "low", "sandbox": {"type": "readOnly"}}
                # A planned answer replaces the settings: a key it leaves out is absent.
                if (cwd / "thread-settings.json").exists():
                    result = {"thread": result["thread"], **json.loads((cwd / "thread-settings.json").read_text())}
            elif method == "turn/start":
                sequence += 1
                active = f"turn-{sequence}"
                turn = active
                emit({"method": "turn/started", "params": {
                    "threadId": thread, "turn": {"id": turn, "status": "inProgress"}}})
                result = {"turn": {"id": turn, "status": "inProgress"}}
                delay = float((cwd / "sleep.txt").read_text()) if (cwd / "sleep.txt").exists() else 0.01
                timer = threading.Timer(delay, complete, [turn])
                timer.daemon = True
                timer.start()
            elif method == "turn/steer":
                if active != params.get("expectedTurnId") or active is None:
                    emit({"id": request["id"], "error": {"code": -32600, "message": "turn mismatch"}})
                    continue
                result = {"turnId": active}
            elif method == "turn/interrupt":
                if timer:
                    timer.cancel()
                complete(params["turnId"], "interrupted")
            elif method == "thread/archive":
                if (cwd / "archive-error.txt").exists():
                    emit({"id": request["id"], "error": {"code": -32600, "message": "archive fixture refused"}})
                    continue
                archive_thread(params["threadId"])
                emit({"method": "thread/archived", "params": {"threadId": thread}})
            emit({"id": request["id"], "result": result})
finally:
    if timer:
        timer.cancel()
