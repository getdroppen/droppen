# Running a relay

`droppen-relay` is one static process: no database and no files (unless
`DROPPEN_RELAY_STATE_DIR` names a volume to keep its key in). Build its image
from the repository root, make it a key, and run it behind a TLS-terminating
proxy:

```sh
docker build -f server-rs/crates/rh-relay/Dockerfile -t droppen-relay .
docker run --rm droppen-relay droppen-relay keygen   # prints DROPPEN_RELAY_KEY=… (a secret)
docker run -p 8080:8080 \
  -e DROPPEN_RELAY_HUB=https://<the Hub's origin> \
  -e DROPPEN_RELAY_ORIGIN=wss://<this relay's public origin> \
  -e DROPPEN_RELAY_KEY=<the key> \
  -e DROPPEN_RELAY_REGISTRATION_TOKEN=<the one-time token the Hub issued for this relay> \
  -e DROPPEN_RELAY_TRUST_PROXY=railway \
  droppen-relay
```

The names from before the rename, `RH_RELAY_*`, are not read since v0.2.135:
set the `DROPPEN_RELAY_*` name with the same value instead.

The Hub accepts the registration token once, on the first start. Keep it set
afterwards all the same, since it also names the relay, or set
`DROPPEN_RELAY_ID` instead. The relay's per-address limits need each client's
own address: behind a proxy, set `DROPPEN_RELAY_TRUST_PROXY=railway` and make
the proxy overwrite `X-Real-IP` with the client's address (never pass on a
value the client sent), or `fly` on Fly.io (`Fly-Client-IP`). Left unset, the
relay takes the socket's address, which behind a proxy is the proxy's for
every client.

Every variable is described in
[`server-rs/crates/rh-relay/src/config.rs`](../server-rs/crates/rh-relay/src/config.rs),
and the protocol between a relay and its Hub in
[relay-and-plans.md](relay-and-plans.md) (in Chinese). A relay pairs
channels only on tickets its Hub signs, so it serves the accounts of the Hub
that registered it. droppen.app lists only the relays it runs: it does not take
relays run by others yet.
