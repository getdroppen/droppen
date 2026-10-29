//! What a device sends the hub and the relays, compared whole: the sign-ins
//! on `/v1/client` and a relay's `/v1/connect`, `stream_unsub`, and the
//! device-session request bodies. What changes per run (clocks,
//! signatures) is blanked after a type check; everything else is exact.

use std::time::Duration;

use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use rh_client::link::Link;
use rh_client::streams::Pages;
use rh_client::{hex, HubUrl};
use rh_proto::record::StreamId;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

const NONCE: [u8; 32] = [5; 32];

/// `v` with each of `keys` checked (a number, or a string that is not empty)
/// and blanked to `0` or `""`.
fn blank(mut v: Value, keys: &[&str]) -> Value {
    for key in keys {
        let was = v[*key].clone();
        v[*key] = match was {
            Value::Number(n) if n.is_u64() => json!(0),
            Value::String(s) if !s.is_empty() => json!(""),
            other => panic!("{key}: {other} in {v}"),
        };
    }
    v
}

/// A websocket server for one connection: it sends a challenge, hands back
/// the first text it gets, and answers it with `welcome`.
async fn sign_in_server() -> (String, tokio::task::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("ws://{}", listener.local_addr().unwrap());
    let served = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let challenge = json!({"t": "challenge", "nonce": hex(&NONCE)}).to_string();
        ws.send(Message::Text(challenge.into())).await.unwrap();
        let login = loop {
            match ws.next().await.unwrap().unwrap() {
                Message::Text(t) => break serde_json::from_str::<Value>(t.as_str()).unwrap(),
                _ => continue,
            }
        };
        ws.send(Message::Text(json!({"t": "welcome"}).to_string().into())).await.unwrap();
        // Held open until the client goes.
        while let Some(Ok(_)) = ws.next().await {}
        login
    });
    (origin, served)
}

#[tokio::test]
async fn a_device_signs_in_on_the_hub_in_the_documented_shape() {
    let (origin, served) = sign_in_server().await;
    let hub = HubUrl::parse(&origin).unwrap();
    let key = SigningKey::from_bytes(&[3; 32]);
    let link = rh_client::link::connect_device(&hub, "a-1", "d-1", &key, "sess-1").await.unwrap();
    drop(link);
    let login = tokio::time::timeout(Duration::from_secs(5), served).await.unwrap().unwrap();
    assert_eq!(
        blank(login, &["time_ms", "sig"]),
        json!({"t": "login", "role": "device", "account": "a-1", "id": "d-1", "time_ms": 0, "sig": "", "session": "sess-1"})
    );
}

#[tokio::test]
async fn a_device_signs_in_on_a_relay_in_the_documented_shape() {
    let (origin, served) = sign_in_server().await;
    let key = SigningKey::from_bytes(&[3; 32]);
    let link = rh_client::link::connect_relay(&origin, "a-1", "d-1", &key, "ticket-1").await.unwrap();
    drop(link);
    let login = tokio::time::timeout(Duration::from_secs(5), served).await.unwrap().unwrap();
    assert_eq!(
        blank(login, &["time_ms", "sig"]),
        json!({"t": "login", "ticket": "ticket-1", "time_ms": 0, "sig": ""})
    );
}

#[tokio::test]
async fn a_stream_read_ends_with_stream_unsub() {
    let (sent, mut texts) = tokio::sync::mpsc::unbounded_channel::<Message>();
    let sink = Box::pin(futures_util::sink::unfold(sent, |sent, m: Message| async move {
        let _ = sent.send(m);
        Ok::<_, tokio_tungstenite::tungstenite::Error>(sent)
    }));
    let link = Link::spawn(sink, futures_util::stream::pending::<Result<Message, tokio_tungstenite::tungstenite::Error>>(), "/v1/client");
    let http = reqwest::Client::new();
    let hub = HubUrl::parse("ws://127.0.0.1:1").unwrap();
    let pages = Pages {
        http: &http,
        hub: &hub,
        token: "tok",
    };
    let read = rh_client::streams::read_stream(&link, pages, [7; 32], &[], StreamId([9; 32]), Duration::from_millis(50)).await;
    assert!(read.is_err(), "nothing came");
    let unsub = loop {
        match tokio::time::timeout(Duration::from_secs(5), texts.recv()).await.unwrap().unwrap() {
            Message::Text(t) => break serde_json::from_str::<Value>(t.as_str()).unwrap(),
            _ => continue,
        }
    };
    assert_eq!(unsub, json!({"t": "stream_unsub", "ch": 1}));
}

/// One HTTP exchange per answer, in order, on one port: each request's line
/// and JSON body come back.
async fn http_server(answers: Vec<Value>) -> (String, tokio::task::JoinHandle<Vec<(String, Value)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("ws://{}", listener.local_addr().unwrap());
    let served = tokio::spawn(async move {
        let mut got = Vec::new();
        for answer in answers {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head, body) = loop {
                let n = tcp.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let head = String::from_utf8_lossy(&buf[..end]).to_string();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                    .unwrap_or(0);
                while buf.len() < end + 4 + len {
                    let n = tcp.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                }
                break (head, buf[end + 4..end + 4 + len].to_vec());
            };
            let text = answer.to_string();
            let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len());
            tcp.write_all(reply.as_bytes()).await.unwrap();
            let line = head.lines().next().unwrap().trim_end_matches(" HTTP/1.1").to_string();
            got.push((line, serde_json::from_slice(&body).unwrap()));
        }
        got
    });
    (origin, served)
}

#[tokio::test]
async fn a_device_session_is_asked_for_in_the_documented_shape() {
    let (origin, served) = http_server(vec![json!({"nonce": hex(&NONCE)}), json!({"token": "tok", "expires_ms": 9})]).await;
    let hub = HubUrl::parse(&origin).unwrap();
    let key = SigningKey::from_bytes(&[3; 32]);
    let minted = rh_client::session::mint(&reqwest::Client::new(), &hub, "a-1", "d-1", &key, 1_000).await.unwrap();
    assert_eq!(minted.expires_ms, Some(9));
    let got = served.await.unwrap();
    assert_eq!(got[0], ("POST /v1/session/challenge".to_string(), json!({"account": "a-1", "device": "d-1"})));
    assert_eq!(got[1].0, "POST /v1/session");
    assert_eq!(
        blank(got[1].1.clone(), &["signature"]),
        json!({"account": "a-1", "device": "d-1", "time_ms": 1000, "nonce": hex(&NONCE), "signature": ""})
    );
}
