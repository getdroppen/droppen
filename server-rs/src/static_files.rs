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
//!
//! Its answers go out compressed to a browser that takes brotli or gzip
//! (tower-http's compression, around this service only). On a LAN, a tailnet
//! or localhost nothing between this server and the browser compresses them,
//! and the JavaScript a page boots from is about 3 MB as it is.

use std::convert::Infallible;
use std::task::{Context, Poll};

use gotcha::axum::body::{Body, HttpBody};
use gotcha::axum::extract::Request;
use gotcha::axum::http::{header, HeaderValue, Response, StatusCode};
use tower_http::compression::predicate::{NotForContentType, SizeAbove};
use tower_http::compression::{Compression, CompressionBody, CompressionLevel, Predicate};
use tower_http::map_response_body::MapResponseBody;
use tower_service::Service;

/// Serves the SPA out of the binary.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpaService;

/// [`SpaService`], its answers compressed for a client that accepts it.
pub type Spa = Compressed<SpaService>;

/// The fallback service for every path the API did not claim.
pub fn service() -> Spa {
    compressed(SpaService)
}

/// `inner`, behind tower-http's compression: brotli or gzip, whichever the
/// request's `Accept-Encoding` weighs higher (every line of it; brotli on a
/// tie), for an answer [`Compressible`] lets through.
///
/// Only the web app's own files go through it, never `/api/*`, the feed and
/// terminal sockets or `/embed/*`: an answer that carries a secret next to
/// text a page can make the server reflect is what BREACH reads through
/// compression, and a stream would be held back by the compressor's buffer.
///
/// On the fly, so at a level a request can afford: 5, brotli and gzip alike.
/// On the app's 2 MB main chunk, brotli 5 takes about 80 ms of one core and
/// comes out 15% larger than brotli 11, which takes seconds; tower-http's
/// default, 4, takes half the time and comes out 25% larger, little better
/// than gzip on the vendor chunk. A file under `/assets/` costs that once per
/// browser, which then keeps it for a year.
pub type Compressed<S> = MapResponseBody<Compression<S, Compressible>, fn(CompressionBody<Body>) -> Body>;

fn compressed<S>(inner: S) -> Compressed<S> {
    // Never zstd, should another crate turn the feature on: it links a C
    // library the Linux and desktop builds do without. Nor deflate, which
    // every browser that sends it sends with gzip.
    let compression = Compression::new(inner)
        .no_zstd()
        .no_deflate()
        .quality(CompressionLevel::Precise(5))
        .compress_when(Compressible);
    MapResponseBody::new(compression, Body::new as fn(CompressionBody<Body>) -> Body)
}

/// Files smaller than this are sent as they are: compressing them saves about
/// what `Content-Encoding` and `Vary` cost. The feed socket's floor too
/// (`feed::mux::COMPRESS_MIN`).
const COMPRESS_MIN: u16 = 1024;

/// Which answers go out compressed: those of [`COMPRESS_MIN`] bytes or more,
/// except images (svg is text) and fonts, which are compressed formats already,
/// and a server-sent event stream, which must reach the page event by event
/// whatever this service comes to answer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Compressible;

impl Predicate for Compressible {
    fn should_compress<B: HttpBody>(&self, response: &Response<B>) -> bool {
        SizeAbove::new(COMPRESS_MIN)
            .and(NotForContentType::IMAGES)
            .and(NotForContentType::const_new("font/"))
            .and(NotForContentType::SSE)
            .and(NotForContentType::GRPC)
            .should_compress(response)
    }
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
        // A `/plugins/…` request the asset route (`crate::plugins::serve`) did
        // not match — too few segments, say — must not be handed the shell for
        // the same reason a missing hashed chunk is not: a module loader would
        // cache `index.html` under the script's name and feed it to `import()`.
        None if trimmed.starts_with("plugins/") => {
            let mut response = json_error(StatusCode::NOT_FOUND, "not found");
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
/// that is what comes back; and the tag of a file this server compresses is
/// weak itself ([`file_response`]).
fn matches_etag(if_none_match: &str, etag: &str) -> bool {
    let etag = etag.trim_start_matches("W/");
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
    let mut response = Response::new(Body::from(body));
    if let Ok(value) = HeaderValue::from_str(&content_type(path)) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    // A file [`compressed`] sends compressed to a client that takes it: then
    // every answer for it depends on `Accept-Encoding` — a `304` too, which
    // the compression layer leaves alone — and its tag is weak, the same for
    // each coding, as nginx makes it: the compressed bytes are not the ones a
    // strong tag of the file would name, only the same content.
    let compressible = Compressible.should_compress(&response);
    let etag = etag.map(|etag| if compressible { format!("W/{etag}") } else { etag });
    if matches!((&etag, if_none_match), (Some(etag), Some(sent)) if matches_etag(sent, etag)) {
        response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
    }
    let cache = if immutable { "public, max-age=31536000, immutable" } else { "no-cache" };
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if compressible {
        response.headers_mut().insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
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
    use gotcha::axum::http::Method;
    use gotcha::axum::Router;

    fn get(path: &str) -> Response<Body> {
        respond(path)
    }

    /// A request as the browser's service-worker update check sends it.
    fn worker_fetch(path: &str, header_value: Option<&str>) -> Response<Body> {
        let mut builder = gotcha::axum::http::Request::builder().uri(path);
        if let Some(value) = header_value {
            builder = builder.header("service-worker", value);
        }
        let mut spa = SpaService;
        Service::call(&mut spa, builder.body(()).unwrap()).into_inner().unwrap()
    }

    /// Text big enough to be compressed.
    fn script() -> Vec<u8> {
        "console.log(\"the file as it is\");\n".repeat(64).into_bytes()
    }

    /// What a client gets for a request with `headers` when the server answers
    /// with `body` as the file at `path`, compression included:
    /// [`file_response`] behind [`compressed`], mounted as the server mounts
    /// it and reached over a real connection (hyper is what sets
    /// `Content-Length` and drops a `HEAD`'s body). The client decodes nothing.
    async fn fetch(method: Method, path: &'static str, body: Vec<u8>, headers: &[(&'static str, &str)]) -> Response<Body> {
        #[derive(Clone)]
        struct File(&'static str, Vec<u8>);
        impl<B> Service<Request<B>> for File {
            type Response = Response<Body>;
            type Error = Infallible;
            type Future = std::future::Ready<Result<Response<Body>, Infallible>>;
            fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
                Poll::Ready(Ok(()))
            }
            fn call(&mut self, request: Request<B>) -> Self::Future {
                let sent = request.headers().get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok());
                std::future::ready(Ok(file_response(self.0, self.1.clone(), false, sent)))
            }
        }
        let router: Router = Router::new().fallback_service(compressed(File(path, body)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { gotcha::axum::serve(listener, router).await });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut request = client.request(method, format!("http://{addr}/{path}"));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let sent = request.send().await.unwrap();
        let mut response = Response::new(Body::empty());
        *response.status_mut() = sent.status();
        *response.headers_mut() = sent.headers().clone();
        *response.body_mut() = Body::from(sent.bytes().await.unwrap());
        response
    }

    async fn body_of(response: Response<Body>) -> Vec<u8> {
        gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    fn gunzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes).read_to_end(&mut out).unwrap();
        out
    }

    fn header_of<'a>(response: &'a Response<Body>, name: header::HeaderName) -> Option<&'a str> {
        response.headers().get(name).map(|value| value.to_str().unwrap())
    }

    #[tokio::test]
    async fn the_web_app_goes_out_in_the_coding_the_client_weighs_highest() {
        let coding = |accept: Option<&'static str>| async move {
            let headers: Vec<_> = accept.map(|accept| ("accept-encoding", accept)).into_iter().collect();
            let response = fetch(Method::GET, "assets/index-abc123.js", script(), &headers).await;
            assert_eq!(response.status(), StatusCode::OK);
            let kind = content_type("assets/index-abc123.js");
            assert_eq!(header_of(&response, header::CONTENT_TYPE), Some(kind.as_str()), "{accept:?}: the file's own type");
            let vary: Vec<_> = response.headers().get_all(header::VARY).iter().map(|value| value.to_str().unwrap()).collect();
            assert_eq!(vary, ["accept-encoding"], "{accept:?}: every answer for the file, once");
            header_of(&response, header::CONTENT_ENCODING).map(str::to_string)
        };
        assert_eq!(coding(Some("gzip, deflate, br, zstd")).await.as_deref(), Some("br"), "a browser over https, or on localhost");
        assert_eq!(coding(Some("gzip, deflate")).await.as_deref(), Some("gzip"), "a browser over plain http");
        assert_eq!(coding(None).await, None, "no header: curl, a script");
        assert_eq!(coding(Some("")).await, None);
        assert_eq!(coding(Some("identity")).await, None);
        assert_eq!(coding(Some("deflate, zstd")).await, None, "never deflate or zstd");
        assert_eq!(coding(Some("br;q=0, gzip")).await.as_deref(), Some("gzip"), "q=0 rules a coding out");
        assert_eq!(coding(Some("br;q=0, gzip;q=0")).await, None);
        assert_eq!(coding(Some("gzip;q=1.0, br;q=0.5")).await.as_deref(), Some("gzip"), "the client's preference first");
        assert_eq!(coding(Some("gzip;q=0.5, br;q=0.5")).await.as_deref(), Some("br"), "brotli on a tie");
        assert_eq!(coding(Some("x-gzip")).await.as_deref(), Some("gzip"));

        let br = fetch(Method::GET, "assets/index-abc123.js", script(), &[("accept-encoding", "br")]).await;
        assert!(br.headers().get(header::CONTENT_LENGTH).is_none(), "streamed as it is compressed");
        assert!(body_of(br).await.len() < script().len());
        let gzip = fetch(Method::GET, "assets/index-abc123.js", script(), &[("accept-encoding", "gzip")]).await;
        assert_eq!(gunzip(&body_of(gzip).await), script(), "the file, compressed");
        let identity = fetch(Method::GET, "assets/index-abc123.js", script(), &[]).await;
        assert_eq!(header_of(&identity, header::CONTENT_LENGTH), Some(script().len().to_string().as_str()));
        assert_eq!(body_of(identity).await, script());
    }

    #[tokio::test]
    async fn accept_encoding_split_over_several_lines_is_one_list() {
        let coding = |lines: &'static [(&'static str, &'static str)]| async move {
            let response = fetch(Method::GET, "assets/index-abc123.js", script(), lines).await;
            header_of(&response, header::CONTENT_ENCODING).map(str::to_string)
        };
        assert_eq!(coding(&[("accept-encoding", "gzip"), ("accept-encoding", "br")]).await.as_deref(), Some("br"), "not the first line alone");
        assert_eq!(coding(&[("accept-encoding", "br;q=0"), ("accept-encoding", "gzip")]).await.as_deref(), Some("gzip"));
        assert_eq!(coding(&[("accept-encoding", "identity"), ("accept-encoding", "br;q=0")]).await, None);
    }

    #[tokio::test]
    async fn a_compressed_files_tag_is_weak_and_the_same_for_every_coding_and_its_304_comes() {
        let shell = |headers: &'static [(&'static str, &'static str)]| fetch(Method::GET, "index.html", script(), headers);
        let strong = etag_of(&script());
        let weak = format!("W/{strong}");
        let codings: [&'static [(&'static str, &'static str)]; 3] = [&[("accept-encoding", "br")], &[("accept-encoding", "gzip")], &[]];
        for headers in codings {
            let response = shell(headers).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(header_of(&response, header::ETAG), Some(weak.as_str()), "{headers:?}: never the strong tag of the bytes as they are");
        }

        let again = fetch(Method::GET, "index.html", script(), &[("accept-encoding", "br"), ("if-none-match", weak.as_str())]).await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header_of(&again, header::ETAG), Some(weak.as_str()));
        assert_eq!(header_of(&again, header::VARY), Some("accept-encoding"), "a 304 varies as its 200 does");
        assert_eq!(header_of(&again, header::CACHE_CONTROL), Some("no-cache"));
        assert!(again.headers().get(header::CONTENT_ENCODING).is_none(), "a 304 has no body to encode");
        assert!(body_of(again).await.is_empty());
        // An older server's strong tag, a list that names it, `*`.
        for sent in [strong.clone(), format!("\"other\", W/\"x\", {weak}"), "*".to_string()] {
            let response = fetch(Method::GET, "index.html", script(), &[("accept-encoding", "gzip"), ("if-none-match", sent.as_str())]).await;
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{sent:?}");
        }
        let stale = fetch(Method::GET, "index.html", script(), &[("accept-encoding", "br"), ("if-none-match", "W/\"stale\"")]).await;
        assert_eq!(stale.status(), StatusCode::OK, "a deploy is never a 304");
        assert_eq!(header_of(&stale, header::CONTENT_ENCODING), Some("br"));

        let tiny = fetch(Method::GET, "index.html", b"<!doctype html>".to_vec(), &[("accept-encoding", "br")]).await;
        assert_eq!(header_of(&tiny, header::ETAG), Some(etag_of(b"<!doctype html>").as_str()), "sent as it is: the strong tag");
        assert!(tiny.headers().get(header::VARY).is_none() && tiny.headers().get(header::CONTENT_ENCODING).is_none());
    }

    /// Images, fonts and files too small to gain go out as they are; an
    /// event stream never goes out compressed, whatever this service comes to
    /// answer.
    #[cfg(feature = "embed")]
    #[tokio::test]
    async fn images_fonts_tiny_files_and_event_streams_are_sent_as_they_are() {
        for (path, body) in [
            ("icon-512.png", vec![0x89; 4096]),
            ("assets/KaTeX_Main-Regular-abc.woff2", vec![7; 4096]),
            ("assets/sheet-abc.js", b"export const x = 1;".to_vec()),
        ] {
            let response = fetch(Method::GET, path, body.clone(), &[("accept-encoding", "br, gzip")]).await;
            assert!(response.headers().get(header::CONTENT_ENCODING).is_none(), "{path}");
            assert!(response.headers().get(header::VARY).is_none(), "{path}: one answer for everyone");
            assert_eq!(body_of(response).await, body, "{path}");
        }
        let svg = fetch(Method::GET, "logo.svg", "<svg><path d=\"M0 0h1v1z\"/></svg>\n".repeat(64).into_bytes(), &[("accept-encoding", "br")]).await;
        assert_eq!(header_of(&svg, header::CONTENT_ENCODING), Some("br"), "svg is text");

        let mut events = Response::new(Body::from("data: {}\n\n".repeat(1024)));
        events.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        assert!(!Compressible.should_compress(&events));
        events.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(Compressible.should_compress(&events), "the same answer as text");
    }

    /// `HEAD` and `Range`, as a client sees them.
    #[tokio::test]
    async fn head_is_the_get_without_its_body_and_a_range_is_the_whole_file() {
        let get = fetch(Method::GET, "index.html", script(), &[("accept-encoding", "gzip, deflate, br")]).await;
        let head = fetch(Method::HEAD, "index.html", script(), &[("accept-encoding", "gzip, deflate, br")]).await;
        assert_eq!(head.status(), StatusCode::OK);
        for name in [header::CONTENT_ENCODING, header::CONTENT_TYPE, header::CONTENT_LENGTH, header::VARY, header::ETAG, header::CACHE_CONTROL] {
            assert_eq!(head.headers().get(&name), get.headers().get(&name), "{name}");
        }
        assert_eq!(header_of(&head, header::CONTENT_ENCODING), Some("br"));
        assert!(body_of(head).await.is_empty());
        let plain = fetch(Method::HEAD, "index.html", script(), &[]).await;
        assert_eq!(header_of(&plain, header::CONTENT_LENGTH), Some(script().len().to_string().as_str()), "as it is, its length");

        let ranged = fetch(Method::GET, "index.html", script(), &[("accept-encoding", "br"), ("range", "bytes=0-0")]).await;
        assert_eq!(ranged.status(), StatusCode::OK, "ranges are not served: the whole file");
        assert!(ranged.headers().get(header::CONTENT_RANGE).is_none());
        assert_eq!(header_of(&ranged, header::CONTENT_ENCODING), Some("br"));
    }

    /// The shell and its scripts out of this build, through the service the
    /// server mounts; only a build with `web/dist` has them.
    #[cfg(all(feature = "embed", not(web_dist_missing)))]
    #[tokio::test]
    async fn the_embedded_shell_goes_out_compressed_from_any_route() {
        let shell = assets::get("index.html").expect("the build's shell");
        let send = |path: &'static str, accept: &'static str| async move {
            let request = gotcha::axum::http::Request::builder().uri(path).header("accept-encoding", accept).body(Body::empty()).unwrap();
            Service::call(&mut service(), request).await.unwrap()
        };
        for route in ["/", "/sessions", "/some/deep/route"] {
            let gzip = send(route, "gzip, deflate").await;
            assert_eq!(header_of(&gzip, header::CONTENT_ENCODING), Some("gzip"), "{route}");
            assert_eq!(header_of(&gzip, header::VARY), Some("accept-encoding"), "{route}");
            assert!(header_of(&gzip, header::ETAG).is_some_and(|etag| etag.starts_with("W/\"")), "{route}");
            assert_eq!(gunzip(&body_of(gzip).await), shell, "{route}: the shell");
            assert_eq!(header_of(&send(route, "gzip, deflate, br, zstd").await, header::CONTENT_ENCODING), Some("br"), "{route}");
        }
        let missing = send("/assets/index-NoSuchHash.js", "br").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert!(missing.headers().get(header::CONTENT_ENCODING).is_none());
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

    /// A `/plugins/…` URL the asset route did not match is a `404`, never the
    /// shell: a module loader would keep the HTML under the chunk's name.
    #[test]
    fn an_unmatched_plugin_url_is_a_404_not_the_shell() {
        for path in ["/plugins/key/tasks", "/plugins/key/tasks/build", "/plugins/"] {
            let response = get(path);
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store", "{path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json", "{path}");
        }
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

    /// The local web UI when a hub splits its hosts (the app on
    /// my.droppen.app, the site on www): this server serves the same build
    /// with no `rh-hub` marker, so the page runs in local mode, and has no
    /// `/.well-known/droppen-hub` of its own — the page never takes this host
    /// for a hub's app or site. Where it signs in comes from its server
    /// (`GET /api/cloud`'s `hubWeb`, `api/hub_web.rs`).
    #[test]
    fn the_local_page_names_no_hub_and_this_server_names_no_hosts() {
        let discovery = get("/.well-known/droppen-hub");
        let kind = discovery.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or_default().to_string());
        assert_ne!(kind.as_deref(), Some("application/json"), "no discovery document here");
        if let Some(shell) = assets::get("index.html") {
            assert_eq!(kind.as_deref(), Some("text/html; charset=utf-8"), "the shell, as for any route");
            assert!(!String::from_utf8_lossy(&shell).contains(r#"name="rh-hub""#), "no hub marker on the local page");
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
