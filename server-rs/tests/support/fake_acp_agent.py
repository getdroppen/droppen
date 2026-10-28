#!/usr/bin/env python3
"""A bounded, local stand-in for an ACP agent; never talks to a real model.

Speaks the part of ACP Droppen uses: initialize, session/new, session/load,
session/set_config_option, session/close, session/prompt and session/cancel.
Its sessions offer a model and a reasoning effort as configuration options,
as real agents' do. Everything it is asked is appended to `agent-calls.jsonl` in its
working directory — or to `$FAKE_AGENT_LOG`, which a probe's scratch directory
needs — so a test can read what it was sent. A prompt containing "wait" holds
its turn open until it is cancelled. `FAKE_AGENT_SLOW=<method>=<seconds>,...`
makes it take that long over a request, after writing it down, so a test can
act while the request is in flight; `FAKE_AGENT_DEAF` the same, but after
answering, without reading what it is sent meanwhile. `FAKE_AGENT_TITLE` is
what it calls itself, and `FAKE_AGENT_FIRST_MODEL` a model it offers before the
others, which its sessions start on.
"""
import json
import os
from pathlib import Path
import sys
import threading
import time

# Started with the arguments the settings give it, and only those.
if sys.argv[1:] != ["--acp", "--stdio"]:
    sys.exit(2)

# A plugin that prints to the wrong stream: not the protocol, and the client
# has to read past it.
print("agent: loading plugins", flush=True)

cwd = Path.cwd()
calls = Path(os.environ["FAKE_AGENT_LOG"]) if os.environ.get("FAKE_AGENT_LOG") else cwd / "agent-calls.jsonl"
lock = threading.Lock()
waiting = {}

MODELS = [("fake:small", "Small"), ("openrouter:vendor/big", "Big")]
if os.environ.get("FAKE_AGENT_FIRST_MODEL"):
    MODELS.insert(0, (os.environ["FAKE_AGENT_FIRST_MODEL"], "First"))
EFFORTS = ["auto", "low", "high"]
# Each session's model and effort, as set_config_option leaves them.
runtime = {}
def pauses(variable):
    return {
        method: float(seconds)
        for method, seconds in (pair.split("=", 1) for pair in os.environ.get(variable, "").split(",") if pair)
    }


slow = pauses("FAKE_AGENT_SLOW")
deaf = pauses("FAKE_AGENT_DEAF")


def config_options(session):
    current = runtime.setdefault(session, {"model": MODELS[0][0], "reasoning_effort": "auto"})
    return [
        {"id": "model", "name": "Model", "category": "model", "type": "select",
         "currentValue": current["model"], "options": [{"value": v, "name": n} for v, n in MODELS]},
        {"id": "reasoning_effort", "name": "Reasoning effort", "category": "thought_level", "type": "select",
         "currentValue": current["reasoning_effort"], "options": [{"value": v, "name": v.title()} for v in EFFORTS]},
        {"id": "fast", "name": "Fast", "type": "boolean", "currentValue": False},
    ]


def log(entry):
    with lock, calls.open("a") as out:
        out.write(json.dumps(entry) + "\n")


def emit(value):
    with lock:
        sys.stdout.write(json.dumps(value) + "\n")
        sys.stdout.flush()


def update(session, body):
    emit({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": body}})


def chunk(kind, text):
    return {"sessionUpdate": kind, "content": {"type": "text", "text": text}}


def answer(request, result):
    emit({"jsonrpc": "2.0", "id": request["id"], "result": result})


def run_turn(request):
    params = request["params"]
    session = params["sessionId"]
    text = params["prompt"][0]["text"]
    if "wait" in text:
        done = threading.Event()
        waiting[session] = done
        update(session, chunk("agent_message_chunk", "Working on it"))
        done.wait(30)
        answer(request, {"stopReason": "cancelled"})
        return
    update(session, chunk("agent_thought_chunk", "Thinking about "))
    update(session, chunk("agent_thought_chunk", text))
    update(session, {"sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "echo hi",
                     "kind": "execute", "status": "in_progress", "rawInput": {"cmd": "echo hi"}})
    update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "completed",
                     "content": [{"type": "content", "content": {"type": "text", "text": "hi"}}]})
    update(session, chunk("agent_message_chunk", "Hello "))
    update(session, chunk("agent_message_chunk", "from the agent"))
    # Context usage arrives after the answer streams, as a real agent may send it.
    update(session, {"sessionUpdate": "usage_update", "used": 4020, "size": 128000})
    answer(request, {"stopReason": "end_turn"})


for raw in sys.stdin:
    request = json.loads(raw)
    method = request.get("method")
    params = request.get("params", {})
    log({"method": method, "params": params, "mark": os.environ.get("FAKE_AGENT_MARK")})
    time.sleep(slow.get(method, 0))
    if method == "session/cancel":
        done = waiting.pop(params.get("sessionId"), None)
        if done:
            done.set()
    elif method == "initialize":
        answer(request, {"protocolVersion": 1, "agentCapabilities": {
            "loadSession": True, "sessionCapabilities": {"close": {}},
            "promptCapabilities": {"image": True, "embeddedContext": True}},
            "agentInfo": {"name": "fake-agent", "title": os.environ.get("FAKE_AGENT_TITLE", "Fake agent"), "version": "0.1.0"}})
    elif method == "session/new":
        session = "f00dfeed0123456789abcdef01234567"
        runtime.pop(session, None)
        answer(request, {"sessionId": session, "configOptions": config_options(session)})
    elif method == "session/set_config_option":
        session, config, value = params["sessionId"], params["configId"], params["value"]
        allowed = {"model": [v for v, _ in MODELS], "reasoning_effort": EFFORTS}.get(config)
        if allowed is None or value not in allowed:
            emit({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32602, "message": f"invalid {config}: {value}"}})
        else:
            config_options(session)
            runtime[session][config] = value
            answer(request, {"configOptions": config_options(session)})
    elif method == "session/close":
        runtime.pop(params["sessionId"], None)
        answer(request, {})
    elif method == "session/load":
        session = params["sessionId"]
        update(session, chunk("user_message_chunk", "an old question"))
        update(session, chunk("agent_message_chunk", "an old answer"))
        answer(request, {"configOptions": config_options(session)})
    elif method == "session/prompt":
        threading.Thread(target=run_turn, args=(request,), daemon=True).start()
    elif "id" in request:
        emit({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown"}})
    time.sleep(deaf.get(method, 0))
