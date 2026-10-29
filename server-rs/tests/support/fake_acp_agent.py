#!/usr/bin/env python3
"""A bounded, local stand-in for an ACP agent; never talks to a real model.

Speaks the part of ACP Droppen uses: initialize, session/new, session/load,
session/set_config_option, session/close, session/prompt and session/cancel.
Its sessions offer a model and a reasoning effort as configuration options,
as real agents' do. Everything it is asked is appended to `agent-calls.jsonl` in its
working directory — or to `$FAKE_AGENT_LOG`, which a probe's scratch directory
needs — so a test can read what it was sent; the arguments and environment it was
started with go to `agent-spawn.json` in its working directory, and every line it
reads, answers included, goes as it came to `agent-stdin.jsonl` beside the call
log. A prompt containing "ask the client" makes it ask the client for a
permission, a file, a terminal and a method nobody offers, waiting for each
answer, and then write one file under each form a request id may take, and some
it may not. A prompt containing "wait" holds
its turn open until it is cancelled; one containing "fail" streams a little
and is answered with an error. `FAKE_AGENT_SLOW=<method>=<seconds>,...`
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
# Renamed into place, so a reader never sees half of it.
spawned = cwd / "agent-spawn.json.tmp"
spawned.write_text(json.dumps({"argv": sys.argv[1:], "env": dict(os.environ)}))
spawned.replace(cwd / "agent-spawn.json")
calls = Path(os.environ["FAKE_AGENT_LOG"]) if os.environ.get("FAKE_AGENT_LOG") else cwd / "agent-calls.jsonl"
stdin_log = calls.parent / "agent-stdin.jsonl"
lock = threading.Lock()
waiting = {}
# The client's answers to what this agent asked it, by request id as JSON.
answers = {}
# The ids the stand-in writes a file under, each named: the forms JSON-RPC
# allows (a string, any number, null) and some it does not.
IDS = [("string", "s-1"), ("big", 9223372036854775808), ("fraction", 1.5), ("null", None),
       ("boolean", True), ("object", {"k": 1}), ("array", [1])]
asked = [0]

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


def ask(method, params):
    with lock:
        asked[0] += 1
        request_id = asked[0]
        answers[json.dumps(request_id)] = [threading.Event(), None]
    emit({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
    answers[json.dumps(request_id)][0].wait(10)
    return answers.pop(json.dumps(request_id))[1] or {}


def ask_the_client(session):
    tool = {"toolCallId": "call-2", "title": "rm -rf build", "kind": "execute", "status": "pending"}
    ask("session/request_permission", {"sessionId": session, "toolCall": tool,
                                       "options": [{"optionId": "once", "name": "Allow once", "kind": "allow_once"}]})
    ask("fs/read_text_file", {"sessionId": session, "path": str(cwd / "notes.txt"), "line": 2, "limit": 1})
    ask("fs/write_text_file", {"sessionId": session, "path": str(cwd / "out.txt"), "content": "written"})
    created = ask("terminal/create", {"sessionId": session, "command": "/bin/sh", "args": ["-c", "printf done; exit 3"]})
    terminal = created.get("result", {}).get("terminalId")
    ask("terminal/wait_for_exit", {"sessionId": session, "terminalId": terminal})
    ask("terminal/output", {"sessionId": session, "terminalId": terminal})
    ask("terminal/release", {"sessionId": session, "terminalId": terminal})
    ask("nope/method", {"sessionId": session})
    # One at a time, each answer is the next one: under the request's own id,
    # or under null where the client refused the id.
    for name, request_id in IDS:
        slot = [threading.Event(), None]
        with lock:
            answers[json.dumps(request_id)] = answers["null"] = slot
        emit({"jsonrpc": "2.0", "id": request_id, "method": "fs/write_text_file",
              "params": {"sessionId": session, "path": str(cwd / f"id-{name}.txt"), "content": name}})
        slot[0].wait(10)
        answers.pop(json.dumps(request_id), None)
        answers.pop("null", None)


def run_turn(request):
    params = request["params"]
    session = params["sessionId"]
    text = params["prompt"][0]["text"]
    if "ask the client" in text:
        ask_the_client(session)
        answer(request, {"stopReason": "end_turn"})
        return
    if "wait" in text:
        done = threading.Event()
        waiting[session] = done
        update(session, chunk("agent_message_chunk", "Working on it"))
        done.wait(30)
        answer(request, {"stopReason": "cancelled"})
        return
    if "fail" in text:
        update(session, chunk("agent_message_chunk", "Trying"))
        emit({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32603, "message": "the model is unavailable"}})
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
    with lock, stdin_log.open("a") as out:
        out.write(raw)
    request = json.loads(raw)
    method = request.get("method")
    if method is None:
        pending = answers.get(json.dumps(request.get("id")))
        if pending:
            pending[1] = request
            pending[0].set()
        continue
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
