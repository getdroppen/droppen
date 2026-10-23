//! Serving the embedded SPA.
//!
//! gotcha exposes no static-file support of its own beyond re-exporting
//! `tower_http`'s `ServeDir`/`ServeFile` behind the `static_files` feature, and
//! its router has no `nest_service` — only
//! [`fallback_service`](gotcha::GotchaRouter::fallback_service)
//! (`gotcha-0.4.1/src/router.rs:236`). That is enough: one tower `Service`
//! placed on the fallback answers every non-API path, resolving assets out of
//! the binary and falling back to `index.html` so client-side routing works on
//! a deep link.

use std::convert::Infallible;
use std::task::{Context, Poll};

use gotcha::axum::body::Body;
use gotcha::axum::extract::Request;
use gotcha::axum::http::{header, HeaderValue, Response, StatusCode};
use tower_service::Service;

/// Serves the SPA out of the binary.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpaService;

/// The fallback service for every path the API did not claim.
pub fn service() -> SpaService {
    SpaService
}

impl<B> Service<Request<B>> for SpaService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        if foreign_worker(request.headers(), request.uri().path()) {
            return std::future::ready(Ok(retiring_worker()));
        }
        let if_none_match = request.headers().get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok());
        std::future::ready(Ok(respond_to(request.uri().path(), request.uri().query(), if_none_match)))
    }
}

/// The page's own service worker: the one address a worker is registered from.
const WORKER_PATH: &str = "/sw.js";

/// What a browser gets when it fetches a service worker from any other
/// address: a worker that empties this origin's caches and unregisters itself.
///
/// One malicious page load can register a worker for the scope `/` from an
/// address of its choosing (`/x.js`). Answered with the shell, as any other
/// path is, the browser's update check fails on the MIME type and the old
/// worker stays — in control of every page, whatever is deployed later. This
/// answer is a valid worker instead, so the update replaces it, and the
/// replacement goes; the page registers `/sw.js` again as it next loads.
/// Droppen Hub answers the same (docs/web-integrity.md).
const RETIRING_WORKER: &str = r#"/* Droppen: no service worker lives at this address. One registered here
 * clears this origin's caches and unregisters itself; the page registers
 * /sw.js again as it next loads. */
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      for (const name of await caches.keys()) await caches.delete(name);
      await self.registration.unregister();
    })(),
  );
});
"#;

/// Whether a request is a browser fetching a service worker script
/// (`Service-Worker: script`) from anywhere but [`WORKER_PATH`].
fn foreign_worker(headers: &gotcha::axum::http::HeaderMap, path: &str) -> bool {
    path != WORKER_PATH && headers.get("service-worker").is_some_and(|value| value.as_bytes() == b"script")
}

fn retiring_worker() -> Response<Body> {
    let mut response = Response::new(Body::from(RETIRING_WORKER));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/javascript; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// One file of the embedded web build, by its path under `web/dist`, or `None`
/// when this binary has no such file (or no web build at all).
///
/// For the server's own reads of what the build produced — today
/// `plugins/manifest.json`, which says which plugins the build bundles
/// ([`crate::plugins`]).
pub fn embedded(path: &str) -> Option<Vec<u8>> {
    assets::get(path.trim_start_matches('/'))
}

#[cfg(test)]
fn respond(path: &str) -> Response<Body> {
    respond_to(path, None, None)
}

fn respond_to(path: &str, query: Option<&str>, if_none_match: Option<&str>) -> Response<Body> {
    // An unmatched `/api/*` is a genuine 404, not a mis-typed SPA route: serving
    // `index.html` there would turn a wrong URL into a confusing HTML body.
    if path.starts_with("/api/") {
        return json_error(StatusCode::NOT_FOUND, "not found");
    }
    let trimmed = path.trim_start_matches('/');
    let version = query.and_then(|query| query.split('&').find_map(|pair| pair.strip_prefix("v=")));
    match assets::get(trimmed) {
        Some(asset) => {
            let versioned = version.is_some_and(|version| content_versioned(trimmed, &asset, version));
            file_response(trimmed, asset, versioned, if_none_match)
        }
        // A hashed asset this build does not have belongs to another build — a
        // tab still running the previous one, asking for a lazy chunk. The
        // shell in its place would be cached by the service worker under the
        // script's name and fed to the module loader as JavaScript; a 404 is
        // what makes the loader say "missing module" and the page reload.
        None if trimmed.starts_with("assets/") => {
            let mut response = Response::new(Body::from("not found"));
            *response.status_mut() = StatusCode::NOT_FOUND;
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        // Any other path is a client-side route: hand back the shell.
        None => match assets::get("index.html") {
            Some(shell) => file_response("index.html", shell, false, if_none_match),
            None => assets::missing_response(),
        },
    }
}

/// Whether `version` (a `?v=`) is this file's own content hash, which makes the
/// URL safe to cache for good: a different file would carry a different one.
///
/// Checked rather than trusted, and only for the images `index.html` and the
/// manifest link that way (`pnpm gen:pwa` writes the first twelve hex digits
/// of their SHA-256 into the links). Never the shell: `/index.html?v=x` must
/// not make the next deploy invisible for a year.
fn content_versioned(path: &str, body: &[u8], version: &str) -> bool {
    use sha2::{Digest, Sha256};
    let image = [".png", ".ico", ".svg", ".webp", ".jpg"].iter().any(|ext| path.ends_with(ext));
    if !image || version.len() < 8 || !version.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let hex: String = Sha256::digest(body).iter().map(|byte| format!("{byte:02x}")).collect();
    hex.starts_with(&version.to_ascii_lowercase())
}

/// A strong validator for a file's bytes: 128 bits of SHA-256, quoted.
fn etag_of(body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(body);
    let hex: String = digest[..16].iter().map(|byte| format!("{byte:02x}")).collect();
    format!("\"{hex}\"")
}

/// Whether an `If-None-Match` names `etag`. Weakly: a proxy that compressed
/// the body on its way out (the tunnel does) hands the browser `W/"…"`, and
/// that is what comes back.
fn matches_etag(if_none_match: &str, etag: &str) -> bool {
    if_none_match
        .split(',')
        .map(|candidate| candidate.trim().trim_start_matches("W/"))
        .any(|candidate| candidate == "*" || candidate == etag)
}

fn file_response(path: &str, body: Vec<u8>, versioned: bool, if_none_match: Option<&str>) -> Response<Body> {
    // Vite writes a content hash into every filename under `assets/`, so those
    // are safe to pin forever, and so is a URL that carries its own content
    // hash as `?v=` (the icons `index.html` links). Everything else — the shell
    // above all, which must never outlive a deploy — is revalidated on every
    // use, but against an `ETag`, so an unchanged file costs a `304` rather than
    // its bytes again: the icons and the shell are most of a reload's requests.
    let immutable = path.starts_with("assets/") || versioned;
    let etag = (!immutable).then(|| etag_of(&body));
    let unchanged = matches!((&etag, if_none_match), (Some(etag), Some(sent)) if matches_etag(sent, etag));
    let mut response = if unchanged {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        response
    } else {
        let mut response = Response::new(Body::from(body));
        if let Ok(value) = HeaderValue::from_str(&content_type(path)) {
            response.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        response
    };
    let cache = if immutable { "public, max-age=31536000, immutable" } else { "no-cache" };
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Some(value) = etag.and_then(|etag| HeaderValue::from_str(&etag).ok()) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

fn json_error(status: StatusCode, message: &str) -> Response<Body> {
    let body = serde_json::json!({ "error": message }).to_string();
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

#[cfg(feature = "embed")]
fn content_type(path: &str) -> String {
    mime_guess::from_path(path).first_or_octet_stream().to_string()
}

#[cfg(not(feature = "embed"))]
fn content_type(_path: &str) -> String {
    "text/html; charset=utf-8".to_string()
}

/// The embedded asset store, or a stub explaining how to produce one.
#[cfg(all(feature = "embed", not(web_dist_missing)))]
mod assets {
    use super::*;

    #[derive(rust_embed::Embed)]
    #[folder = "../web/dist"]
    struct Dist;

    pub fn get(path: &str) -> Option<Vec<u8>> {
        Dist::get(path).map(|file| file.data.into_owned())
    }

    pub fn missing_response() -> Response<Body> {
        json_error(StatusCode::NOT_FOUND, "not found")
    }
}

/// `web/dist` was absent at build time (see `build.rs`), so there is nothing to
/// embed. The API still works; the SPA route explains what to run.
#[cfg(any(not(feature = "embed"), web_dist_missing))]
mod assets {
    use super::*;

    pub fn get(_path: &str) -> Option<Vec<u8>> {
        None
    }

    pub fn missing_response() -> Response<Body> {
        let mut response = Response::new(Body::from(
            "<!doctype html><meta charset=utf-8><title>Droppen</title>\
             <p>The web app was not embedded in this binary. \
             Run <code>pnpm --filter @remote-hand/web build</code> and rebuild.",
        ));
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> Response<Body> {
        respond(path)
    }

    /// A request as the browser's service-worker update check sends it.
    fn worker_fetch(path: &str, header_value: Option<&str>) -> Response<Body> {
        let mut builder = gotcha::axum::http::Request::builder().uri(path);
        if let Some(value) = header_value {
            builder = builder.header("service-worker", value);
        }
        let mut spa = service();
        Service::call(&mut spa, builder.body(()).unwrap()).into_inner().unwrap()
    }

    #[test]
    fn a_worker_fetched_from_another_address_is_one_that_unregisters_itself() {
        for path in ["/x.js", "/assets/evil.js", "/sessions", "/sw.js.map"] {
            let response = worker_fetch(path, Some("script"));
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "text/javascript; charset=utf-8", "{path}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store", "{path}");
        }
        assert!(RETIRING_WORKER.contains("self.registration.unregister()"));
        assert!(RETIRING_WORKER.contains("caches.delete(name)"));
        // The page's own worker, and the same path fetched as anything else, are served as before.
        let mut asked = gotcha::axum::http::HeaderMap::new();
        asked.insert("service-worker", HeaderValue::from_static("script"));
        assert!(!foreign_worker(&asked, "/sw.js"));
        assert!(foreign_worker(&asked, "/x.js"));
        let plain = worker_fetch("/x.js", None);
        assert_ne!(plain.headers().get(header::CACHE_CONTROL).map(|v| v.as_bytes()), Some(&b"no-store"[..]), "a plain fetch is not the retiring worker");
        let other = worker_fetch("/x.js", Some("other"));
        assert_ne!(other.headers().get(header::CONTENT_TYPE).map(|v| v.as_bytes()), Some(&b"text/javascript; charset=utf-8"[..]));
    }

    #[test]
    fn an_unmatched_api_path_is_a_json_404_not_the_spa_shell() {
        let response = get("/api/nope");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    }

    // Chrome ignores a manifest served as anything but JSON, and the extension
    // is obscure enough that a `mime_guess` table change would go unnoticed.
    #[cfg(feature = "embed")]
    #[test]
    fn the_manifest_keeps_its_own_media_type() {
        assert_eq!(content_type("manifest.webmanifest"), "application/manifest+json");
    }

    #[test]
    fn hashed_assets_are_immutable_and_the_shell_is_not() {
        // Only meaningful with a real `web/dist`; without one both fall through
        // to the stub, which is itself the behaviour under test there.
        if assets::get("index.html").is_none() {
            assert_eq!(get("/some/route").status(), StatusCode::SERVICE_UNAVAILABLE);
            return;
        }
        assert_eq!(get("/some/deep/route").headers()[header::CACHE_CONTROL], "no-cache");
    }

    #[test]
    fn a_revalidated_file_that_has_not_changed_is_a_304() {
        let body = b"<!doctype html><title>shell</title>".to_vec();
        let first = file_response("index.html", body.clone(), false, None);
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CACHE_CONTROL], "no-cache");
        let etag = first.headers()[header::ETAG].to_str().unwrap().to_string();

        let again = file_response("index.html", body.clone(), false, Some(&etag));
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED, "the browser already has these bytes");
        assert_eq!(again.headers()[header::ETAG], etag.as_str());
        let weak = format!("W/{etag}");
        assert_eq!(
            file_response("index.html", body.clone(), false, Some(&weak)).status(),
            StatusCode::NOT_MODIFIED,
            "a proxy that compressed the body weakens the tag; it still names the same bytes"
        );
        let changed = file_response("index.html", b"<!doctype html><title>new build</title>".to_vec(), false, Some(&etag));
        assert_eq!(changed.status(), StatusCode::OK, "a deploy is never a 304");
    }

    #[test]
    fn only_an_image_whose_v_is_its_own_hash_is_pinned() {
        use sha2::{Digest, Sha256};
        let body = b"\x89PNG fake".to_vec();
        let hash: String = Sha256::digest(&body).iter().map(|byte| format!("{byte:02x}")).collect();
        assert!(content_versioned("icon-192.png", &body, &hash[..12]));
        assert!(!content_versioned("icon-192.png", &body, "0123456789ab"), "a stale or made-up hash");
        assert!(!content_versioned("icon-192.png", &body, "x"), "not a hash at all");
        assert!(!content_versioned("index.html", &body, &hash[..12]), "never the shell");
        assert!(!content_versioned("manifest.webmanifest", &body, &hash[..12]));
    }

    #[test]
    fn a_missing_hashed_asset_is_a_404_not_the_shell() {
        let response = get("/assets/index-NoSuchHash.js");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            response.headers().get(header::CONTENT_TYPE).is_none(),
            "nothing a loader could take for a script"
        );
    }

    /// Local sign-in (local-aggregator §3.4): the settings page opens the
    /// droppen.app popup and talks to it through `window.opener`, which a
    /// `Cross-Origin-Opener-Policy` on the computer's page would cut.
    #[test]
    fn no_page_cuts_its_opener() {
        for response in [
            get("/settings"),
            get("/"),
            file_response("index.html", b"<!doctype html>".to_vec(), false, None),
        ] {
            assert!(response.headers().get("cross-origin-opener-policy").is_none());
            assert!(response.headers().get("cross-origin-embedder-policy").is_none());
        }
    }

    #[test]
    fn a_url_carrying_its_own_content_hash_is_immutable() {
        let icon = file_response("icon-192.png", vec![1, 2, 3], true, None);
        assert_eq!(icon.headers()[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        assert!(icon.headers().get(header::ETAG).is_none(), "nothing to revalidate");
        let bare = file_response("icon-192.png", vec![1, 2, 3], false, None);
        assert_eq!(bare.headers()[header::CACHE_CONTROL], "no-cache", "the same file without ?v= may change");
        assert!(matches_etag("\"a\", W/\"b\"", "\"b\""));
        assert!(!matches_etag("\"a\"", "\"b\""));
    }
}
