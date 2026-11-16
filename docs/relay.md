# Running a relay

`droppen-relay` is one static process: no database and no files (unless
`DROPPEN_RELAY_STATE_DIR` names a volume to keep its key in). Build its image
from the repository root, make it a key, and run it behind a TLS-terminating
proxy:

```sh
docker build -f crates/droppen-relay-core/Dockerfile -t droppen-relay .
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
[`crates/droppen-relay-core/src/config.rs`](../crates/droppen-relay-core/src/config.rs),
and the protocol between a relay and its Hub in
[relay-and-plans.md](relay-and-plans.md) (in Chinese). A relay pairs
channels only on tickets its Hub signs, so it serves the accounts of the Hub
that registered it. droppen.app lists only the relays it runs: it does not take
relays run by others yet.

## Moving a relay to a new origin

A relay's public origin (`DROPPEN_RELAY_ORIGIN`) can be changed, in this
order:

1. Make the **new** origin reach the relay (DNS and TLS) while the old one
   still does. The Hub proves the new origin before it takes it, and clients
   are still on the old one.
2. Set `DROPPEN_RELAY_ORIGIN` to the new origin and restart the relay. On its
   sign-in the relay names its origin to the Hub (`meta`); the Hub fetches
   `/v1/relay-id` at the new origin and checks it against the key it pinned,
   then updates its directory, and devices and agents get the new origin from
   the directory.
3. Once nothing dials the old origin any more, drop it.

There is a short blip after step 2: the restarted relay refuses sign-ins that
name the old origin, so a client that still holds it is refused until it has
fetched the directory again. If the Hub cannot prove the new origin, it keeps
the old one and the relay logs a warning, "the Hub kept this relay's old
origin": the blip then lasts until you make the new origin reach the relay and
restart it, or set the old origin back. The relay's link to the Hub stays up
meanwhile. The same sign-in also tells the Hub which release the relay is.
