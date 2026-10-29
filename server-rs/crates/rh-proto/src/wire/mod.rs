//! The JSON control messages on the Hub's and the relays' websockets, one
//! module per channel, with the JSON bodies the same parties exchange over
//! HTTP.
//!
//! The contract is `docs/hub-api/` (`index.json`, `schema/ws.schema.json`);
//! these are the types both ends of a channel use for it. A message type is
//! named after who sends it to whom: on `/v1/relay` the relay sends
//! [`relay::RelayToHub`]; on `/v1/connect` an agent or a device sends
//! [`connect::PeerToRelay`] and the relay sends [`connect::RelayToPeer`];
//! on `/v1/client` a device sends [`client::DeviceToHub`].
//! `docs/hub-api/check.mjs` takes a message's channel and direction from
//! its module and its type's name, whichever of `Serialize` and
//! `Deserialize` the type derives, so every enum tagged `t` here must be
//! one of the names that script lists for its module.
//!
//! A message is `{"t": "<its variant in snake_case>", …fields}`. Nothing
//! here is signed as JSON: a signature a message carries is the hex of one
//! over an rh-proto binary encoding ([`crate::sig`]).

pub mod client;
pub mod connect;
pub mod relay;
