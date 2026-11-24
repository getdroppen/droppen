# A2A compatibility checks

Droppen's local program serves A2A v1.0 (`docs/a2a.md`). Its server side is checked against the official A2A TCK ([a2aproject/a2a-tck](https://github.com/a2aproject/a2a-tck)), run against the `tck` entry: a `source: "tck"` entry that runs no CLI and plays the TCK's scenarios by message id (`crates/droppen-a2a/src/tck.rs`). Such an entry can only be made on an instance started with `DROPPEN_A2A_TCK=1`.

Everything else the TCK exercises is the server's own: tokens and their scope, the store, the two bindings, the streams and the errors.

## Running it

```bash
scripts/a2a-compat/tck.sh                 # MUST level (the TCK's mandatory requirements)
scripts/a2a-compat/tck.sh --level all     # every level
scripts/a2a-compat/tck.sh -- -k streaming # extra pytest args go after --
```

**Requirements:** `git`, `curl`, `python3`, [`uv`](https://github.com/astral-sh/uv) (the TCK wants Python ≥ 3.15, which uv downloads), and `cargo` when the script starts the server itself.

What the script does, under `$A2A_COMPAT_DIR` (default `/tmp/droppen-a2a`), never in the repository:

1. It checks out a2a-tck at the pinned revision (`TCK_REV`), then runs `uv venv --python 3.15` and `uv pip install -e .`.
2. It builds the server and starts it on `127.0.0.1:4251` (`TCK_DROPPEN_PORT`) with an instance of its own, `DROPPEN_A2A_TCK=1`, and authentication off for the management API only. `/a2a/*` asks for its A2A token whatever that switch says. The server ends with the script, and by itself after `LIFETIME_S` (1500) seconds.
3. It makes the `tck` entry and a token through `/api/a2a/*`.
4. It runs `run_tck.py --sut-host http://127.0.0.1:4251/a2a/agents/tck --level must -- -p a2a_bearer_plugin`.
   - The TCK has no option for the agent's credentials, so `a2a_bearer_plugin.py` adds `Authorization: Bearer <token>` to every httpx client the TCK opens.
   - A2A tokens are only accepted from loopback peers, which the TCK is.
5. Reports go to `$A2A_COMPAT_DIR/a2a-tck/reports/`: `compatibility.json`, `compatibility.html` and JUnit XML.

### Against a server built elsewhere

Droppen's Rust is built in the build pod, and the pod has no uv. The run below was made this way:

1. Build the server in the pod and start it there detached, with a hard lifetime. A process started from one `kubectl exec` cannot be signalled from another, so the lifetime is what stops it:

   ```bash
   DROPPEN_DATA_DIR=… DROPPEN_CONFIG_DIR=… APP_INSTANCE=a2a-tck APP_SERVER__HOST=127.0.0.1 \
     APP_SERVER__PORT=4252 APP_AUTH__ENABLED=false DROPPEN_A2A_TCK=1 TCK_STREAMING_TIMEOUT=2 \
     DROPPEN_ALLOW_MISSING_WEB_DIST=1 setsid nohup timeout 1500 droppen-server serve &
   ```

2. Make the entry and the token with `curl` against `/api/a2a/entries` and `/api/a2a/tokens`, as the script does.
3. Forward the port: `kubectl port-forward pod/<pod> 4252:4252`. The forward arrives on the pod's loopback, so the token is accepted.
4. On the Mac, run the script with the forwarded base and the token. It then only checks out and runs the TCK:

   ```bash
   TCK_SUT_BASE=http://localhost:4252 A2A_BEARER_TOKEN=dpa_… scripts/a2a-compat/tck.sh
   ```

## Results (2026-11-25, a2a-tck `5e6f37c`, `feat/a2a-core`)

| Run | pytest | MUST passed / failed / skipped / manual / not tested |
| --- | --- | --- |
| `--level must` | **142 passed, 2 failed**, 85 skipped | 77 / 1 / 14 / 7 / 16 |
| `--level all` | **162 passed, 2 failed**, 97 skipped, 2 xfailed | 77 / 1 / 14 / 7 / 16 (SHOULD 9/0/1, MAY 2/1/1) |

By transport (`--level all`):
- agent card: 9/11
- JSON-RPC: 87/96
- HTTP+JSON: 84/92
- gRPC: all skipped (Droppen does not serve gRPC)

The one MUST failure is CORE-MULTI-002a, on both bindings; it is the TCK's, not Droppen's (below). The two xfails are the MAY CORE-MULTI-002 on both bindings.

The 97 skips (`--level all`) break down as follows:

| Skip reason | Count |
| --- | --- |
| gRPC not offered | 68 |
| No `extendedAgentCard` | 6 |
| The TCK's webhook (`http://localhost:<port>`) is not among the entry's `pushOrigins` (PUSH-DELIVER-*) | 6 |
| Push notifications are supported, so "not supported" errors cannot be triggered | 5 |
| Streaming is supported, so "streaming unsupported" errors cannot be triggered | 4 |
| No requirements at the selected level | 3 |
| No required extension | 2 |
| No custom binding | 1 |
| No optional `Last-Modified` header | 1 |
| A wrong `Content-Type` on an HTTP+JSON request is read anyway | 1 |

"Manual" requirements (signing, TLS, client tenant) cannot be automated. The 16 "not tested" MUSTs have no tests in this TCK revision: AUTH-INTASK-*, AUTH-SCOPE-*, AUTH-SERVER-002, BIND-EQUIV-*, VER-CLIENT-*, VER-SERVER-001 and GRPC-SVC-003. The server's own tests cover the scope (`crates/droppen-a2a/src/tests.rs`: another caller's task is not found, exactly as none) and binding equivalence (`both_bindings_answer_alike`).

### CORE-MULTI-002a / CORE-MULTI-002: a client's own contextId

- The requirement (MUST) says: "If an agent cannot accept a client-provided contextId, it MUST reject the request with an error." Droppen does exactly that.
  - A contextId must be one Droppen gave this caller on this entry: one session (`docs/a2a.md` §1, §3.1).
  - Any other contextId is refused with `InvalidParamsError` (-32602 / 400), and no new context is made.
- The TCK checks this requirement with its generic test, which treats every error as a failure (no `expected_error` is set for it). So an agent that rejects the contextId, as the requirement asks, fails it. An agent that accepts it, as dont-do's SDK does, passes it.
- The MAY CORE-MULTI-002 ("Agent may accept client-provided contextId") fails for the same reason. That one is expected: Droppen does not take a client's contextIds.

### Push delivery

- The CRUD requirements pass.
- PUSH-DELIVER-* are skipped: the TCK puts its local webhook receiver's `http://localhost:<port>` URL in the send's configuration, and the entry did not list that origin.
- A `tck` entry of a TCK instance may list loopback `http` origins (`docs/a2a.md` §6). Even listed, nothing delivers pushes before M1c.
- In the forwarded setup the receiver runs on the Mac, which the pod cannot reach.

### What the TCK needed from Droppen (all legitimate behaviour)

- **SUT scenarios.** The `tck` entry plays the TCK's `scenarios/*.feature`, keyed on the message id: `tck-complete-task`, `tck-artifact-*`, `tck-message-response`, `tck-input-required`, `tck-reject-task`, `tck-stream-*` and `test-resubscribe-message-id`.
  - Only a `tck` entry plays them, and only on an instance started with `DROPPEN_A2A_TCK=1`.
  - The TCK sends one message id many times in a run, so a `tck` entry does not tie a `(caller, messageId)` to one task.
- **A subscription to a task waiting on an answer stays open** (STREAM-SUB-002).
  - A stream closes when the task *reaches* a terminal or interrupted state.
  - One opened on a task already in `INPUT_REQUIRED` waits for what comes after the answer.
- **Proto field names in JSON-RPC params.** The TCK's JSON-RPC client sends `history_length`, `context_id`, `task_id`. ProtoJSON readers accept both spellings, and so does Droppen.
- **The trailing-slash JSON-RPC route.** The TCK posts to `<interface url>/`.
- **`A2A-Version: 1.0`** on every call. A missing or empty one is read as 0.3 and refused (VER-SERVER-003 accepts a well-formed error).
