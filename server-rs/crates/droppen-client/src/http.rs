//! JSON requests to the hub's HTTP routes (`docs/hub-api/http.md`).
//!
//! Every JSON body sent and every answer goes past [`crate::tap`]; binary
//! answers (a snapshot page) never do.

use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::tap;

/// Why a request to the hub failed.
#[derive(Debug)]
pub enum HttpError {
    /// No answer: the network, a timeout, a body that would not read.
    Network(reqwest::Error),
    /// The hub answered, with a status that is not a success.
    Status {
        method: Method,
        url: String,
        status: StatusCode,
        /// Its JSON (`{"error": code, …}`), or `null`.
        body: Value,
    },
    /// A success that does not carry what it should, or a request this side
    /// could not make (a signature it could not produce).
    Answer(String),
}

impl HttpError {
    /// The hub's status, if it answered.
    pub fn status(&self) -> Option<u16> {
        match self {
            HttpError::Status { status, .. } => Some(status.as_u16()),
            _ => None,
        }
    }

    /// The hub's error code (`{"error": code}`), if it answered with one.
    pub fn code(&self) -> Option<&str> {
        match self {
            HttpError::Status { body, .. } => body["error"].as_str(),
            _ => None,
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Network(e) => write!(f, "request: {e}"),
            HttpError::Status { method, url, status, body } => write!(f, "{method} {url}: {status} {body}"),
            HttpError::Answer(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for HttpError {}

impl From<HttpError> for String {
    fn from(e: HttpError) -> String {
        e.to_string()
    }
}

/// One request, its answer unjudged: the status and the JSON (`null` when
/// there was none, or it was not JSON). `token`: a bearer session.
pub async fn request(
    http: &reqwest::Client,
    method: Method,
    url: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Result<(StatusCode, Value), HttpError> {
    let mut req = http.request(method.clone(), url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    if let Some(body) = body {
        tap::http_out(method.as_str(), url, body);
        req = req.json(body);
    }
    let res = req.send().await.map_err(HttpError::Network)?;
    let status = res.status();
    let text = res.text().await.map_err(HttpError::Network)?;
    let value = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    tap::http_in(method.as_str(), url, status.as_u16(), &value);
    Ok((status, value))
}

/// [`request`], a status that is not a success an error.
pub async fn send(http: &reqwest::Client, method: Method, url: &str, token: Option<&str>, body: Option<&Value>) -> Result<Value, HttpError> {
    let (status, value) = request(http, method.clone(), url, token, body).await?;
    if !status.is_success() {
        return Err(HttpError::Status {
            method,
            url: url.to_string(),
            status,
            body: value,
        });
    }
    Ok(value)
}

/// `POST url` with a JSON body.
pub async fn post(http: &reqwest::Client, url: &str, token: Option<&str>, body: &Value) -> Result<Value, HttpError> {
    send(http, Method::POST, url, token, Some(body)).await
}

/// `GET url`, signed in.
pub async fn get(http: &reqwest::Client, url: &str, token: &str) -> Result<Value, HttpError> {
    send(http, Method::GET, url, Some(token), None).await
}

/// `DELETE url`, signed in.
pub async fn delete(http: &reqwest::Client, url: &str, token: &str) -> Result<Value, HttpError> {
    send(http, Method::DELETE, url, Some(token), None).await
}
