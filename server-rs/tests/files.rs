//! The filesystem endpoints, against a throwaway home directory.
//!
//! The containment cases are the point of this file: `/api/fs/*` hands the
//! client the server's own filesystem, so the boundary is the only thing between
//! a path in a query string and the user's whole disk. Each case below asserts
//! what the **TypeScript** does rather than what a stricter server might — the
//! two implementations are meant to answer the same request identically, and a
//! difference discovered by a user is a difference discovered too late.

mod common;

use std::path::Path;
use std::time::{Duration, SystemTime};

use common::TestServer;
use serde_json::Value;

/// The boundary marker the hand-built multipart bodies below use.
const MULTIPART_TYPE: &str = "multipart/form-data; boundary=boundary";

/// A multipart body carrying a single `file` field.
///
/// Built by hand rather than with a client crate: the shape is four lines, and a
/// literal body is also the clearest statement of what the server must accept.
fn multipart(filename: &str, content: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\r\n").as_bytes());
    body.extend_from_slice(content);
    body.extend_from_slice(b"\r\n--boundary--\r\n");
    body
}

/// The `error` field of a failure body.
fn error_of(body: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("error body is not JSON ({e}): {body}"));
    value["error"].as_str().unwrap_or_default().to_string()
}

/// `POST path` with a one-field multipart body.
async fn post_file(server: &TestServer, path: &str, filename: &str, content: &[u8]) -> (u16, String) {
    server
        .request("POST", path, &[("Content-Type", MULTIPART_TYPE)], &multipart(filename, content))
        .await
}

#[tokio::test]
async fn a_traversal_out_of_the_home_directory_is_refused() {
    let server = TestServer::start().await;

    let escape = format!("{}/../..", server.home.display());
    let (status, body) = server.get(&format!("/api/fs/list?path={escape}")).await;
    assert_eq!(status, 400, "a `..` escape must not list: {body}");
    assert_eq!(error_of(&body), "path is outside the home directory");

    // The same guard on the other three read paths, since each resolves its own.
    let (status, body) = server.get(&format!("/api/fs/read?path={escape}/etc/hosts")).await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "path is outside the home directory");

    let (status, body) = server.get(&format!("/api/fs/download?path={escape}/etc/hosts")).await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "path is outside the home directory");

    let (status, body) = post_file(&server, &format!("/api/fs/upload?dir={escape}"), "x.txt", b"x").await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "path is outside the home directory");

    server.stop().await;
}

#[tokio::test]
async fn an_absolute_path_outside_the_home_directory_is_refused() {
    let server = TestServer::start().await;

    let (status, body) = server.get("/api/fs/list?path=/etc").await;
    assert_eq!(status, 400, "an absolute path outside the home must not list: {body}");
    assert_eq!(error_of(&body), "path is outside the home directory");

    // A sibling directory whose name merely starts with the home's must not pass
    // a prefix test: containment compares whole path components.
    let sibling = format!("{}-elsewhere", server.home.display());
    let (status, body) = server.get(&format!("/api/fs/list?path={sibling}")).await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "path is outside the home directory");

    server.stop().await;
}

#[tokio::test]
async fn a_symlink_inside_the_home_directory_may_point_outside_it() {
    // This asserts the TypeScript's behaviour, not an idealised one: `resolveSafe`
    // uses Node's `path.resolve`, which collapses segments textually and never
    // touches the filesystem, so containment is decided on the path the client
    // typed. A symlink the home directory's own owner planted therefore resolves
    // and is served. Reproducing that keeps the two servers answering alike; a
    // port that "fixed" it would break whatever setup relies on such a link, and
    // it would do so silently.
    let outside = tempfile::tempdir().expect("tempdir");
    std::fs::write(outside.path().join("beyond.txt"), "outside the home").expect("seed");
    let outside_path = outside.path().to_path_buf();

    let server = TestServer::start_with(|home| {
        std::os::unix::fs::symlink(&outside_path, home.join("escape")).expect("symlink");
    })
    .await;

    let requested = format!("{}/escape/beyond.txt", server.home.display());
    let body = server.get_json(&format!("/api/fs/read?path={requested}")).await;
    assert_eq!(body["content"], "outside the home", "lexical containment lets a symlink through");
    assert_eq!(body["binary"], false);

    server.stop().await;
}

#[tokio::test]
async fn a_listing_puts_directories_first_and_stops_at_the_home_directory() {
    let server = TestServer::start_with(|home| {
        std::fs::create_dir(home.join("zeta-dir")).expect("dir");
        std::fs::write(home.join("alpha.txt"), "a").expect("file");
        std::fs::write(home.join("beta.txt"), "bb").expect("file");
        std::os::unix::fs::symlink(home.join("alpha.txt"), home.join("link")).expect("symlink");
    })
    .await;

    let body = server.get_json("/api/fs/list").await;
    assert_eq!(
        body["path"],
        server.home.to_string_lossy().into_owned(),
        "no `path` means the home directory"
    );
    assert_eq!(body["parent"], Value::Null, "the home directory is the browser's ceiling");

    // `.local` and `.config` hold server data/config, created at boot along with
    // the instance it locks; it is a real entry of the home directory and is
    // dropped here only so the ordering assertion is about the fixture.
    let entries: Vec<&Value> = body["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter(|entry| entry["name"] != ".local" && entry["name"] != ".config")
        .collect();
    let names: Vec<&str> = entries.iter().map(|e| e["name"].as_str().expect("name")).collect();
    assert_eq!(
        names,
        vec!["zeta-dir", "alpha.txt", "beta.txt", "link"],
        "directories first, then by name"
    );

    assert_eq!(entries[0]["kind"], "dir");
    assert_eq!(entries[0]["sizeBytes"], Value::Null, "only regular files carry a size");
    assert_eq!(entries[0]["mtime"], Value::Null);
    assert_eq!(entries[1]["kind"], "file");
    assert_eq!(entries[1]["sizeBytes"], 1);
    assert!(entries[1]["mtime"].as_str().expect("mtime").ends_with('Z'), "mtime is ISO-8601");
    assert_eq!(entries[3]["kind"], "symlink", "a symlink is reported as itself, not followed");
    assert_eq!(entries[3]["sizeBytes"], Value::Null);

    // A directory below the home directory reports its parent.
    let nested = format!("{}/zeta-dir", server.home.display());
    let body = server.get_json(&format!("/api/fs/list?path={nested}")).await;
    assert_eq!(body["parent"], server.home.to_string_lossy().into_owned());

    // A directory that is not there is a 400 with the route's fallback text.
    let missing = format!("{}/nope", server.home.display());
    let (status, body) = server.get(&format!("/api/fs/list?path={missing}")).await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "cannot list directory");

    server.stop().await;
}

#[tokio::test]
async fn reading_covers_text_binary_and_oversized_files() {
    // 300 KiB, so the 256 KiB cap bites and `truncated` has something to report.
    let big = "a".repeat(300 * 1024);
    let big_len = big.len();
    let server = TestServer::start_with(|home| {
        std::fs::write(home.join("note.txt"), "hello, world\n").expect("text");
        std::fs::write(home.join("blob.bin"), [0x1f, 0x8b, 0x00, 0x41, 0x42]).expect("binary");
        std::fs::write(home.join("big.txt"), &big).expect("big");
        std::fs::create_dir(home.join("adir")).expect("dir");
    })
    .await;
    let home = server.home.display().to_string();

    let text = server.get_json(&format!("/api/fs/read?path={home}/note.txt")).await;
    assert_eq!(text["content"], "hello, world\n");
    assert_eq!(text["sizeBytes"], 13);
    assert_eq!(text["binary"], false);
    assert_eq!(text["truncated"], false);
    assert!(text["mtime"].as_str().expect("mtime").ends_with('Z'));

    let binary = server.get_json(&format!("/api/fs/read?path={home}/blob.bin")).await;
    assert_eq!(binary["binary"], true, "a NUL byte in the slice makes it binary");
    assert_eq!(binary["content"], "", "a binary file sends no text");
    assert_eq!(binary["sizeBytes"], 5);

    let oversized = server.get_json(&format!("/api/fs/read?path={home}/big.txt")).await;
    assert_eq!(oversized["truncated"], true);
    assert_eq!(oversized["sizeBytes"], big_len, "the size is the file's, not the slice's");
    assert_eq!(oversized["content"].as_str().expect("content").len(), 256 * 1024);

    // A directory is not readable, and says so in the TypeScript's words.
    let (status, body) = server.get(&format!("/api/fs/read?path={home}/adir")).await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "not a regular file");

    let (status, body) = server.get(&format!("/api/fs/read?path={home}/absent.txt")).await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "cannot read file");

    server.stop().await;
}

#[tokio::test]
async fn a_download_streams_the_file_and_refuses_a_directory() {
    let server = TestServer::start_with(|home| {
        std::fs::write(home.join("report.pdf"), "%PDF-1.4 pretend").expect("file");
    })
    .await;
    let home = server.home.display().to_string();

    // The body arrives chunked (no `Content-Length`), which the harness rejoins.
    let (status, body) = server.get(&format!("/api/fs/download?path={home}/report.pdf")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, "%PDF-1.4 pretend");

    let (status, body) = server.get(&format!("/api/fs/download?path={home}")).await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "not a regular file");

    server.stop().await;
}

#[tokio::test]
async fn an_attachment_lands_in_its_own_uuid_directory() {
    let server = TestServer::start().await;

    let (status, body) = post_file(&server, "/api/uploads", "shot.PNG", b"not really a png").await;
    assert_eq!(status, 200, "{body}");
    let first: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(first["name"], "shot.PNG");
    assert_eq!(first["sizeBytes"], 16);
    assert_eq!(first["isImage"], true, "the extension test is case-insensitive");

    let first_path = Path::new(first["path"].as_str().expect("path"));
    assert!(
        first_path.starts_with(server.paths.uploads_dir()),
        "{first_path:?} escaped the uploads directory"
    );
    assert_eq!(std::fs::read_to_string(first_path).expect("written"), "not really a png");

    // The same name again: a fresh uuid directory, so nothing is overwritten.
    let (status, body) = post_file(&server, "/api/uploads", "shot.PNG", b"a different png").await;
    assert_eq!(status, 200, "{body}");
    let second: Value = serde_json::from_str(&body).expect("json");
    let second_path = Path::new(second["path"].as_str().expect("path"));
    assert_ne!(first_path, second_path, "a second upload must not collide with the first");
    assert_eq!(
        first_path.parent().map(Path::to_path_buf).expect("parent").parent(),
        second_path.parent().expect("parent").parent()
    );
    assert_eq!(std::fs::read_to_string(first_path).expect("still there"), "not really a png");
    assert_eq!(std::fs::read_to_string(second_path).expect("written"), "a different png");

    // A name with no image extension is stored all the same, just not flagged.
    let (status, body) = post_file(&server, "/api/uploads", "notes.md", b"# hi").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["isImage"], false);

    server.stop().await;
}

#[tokio::test]
async fn an_upload_never_overwrites_an_existing_file() {
    let server = TestServer::start_with(|home| {
        std::fs::write(home.join("taken.txt"), "the original").expect("file");
    })
    .await;
    let dir = server.home.display().to_string();

    let (status, body) = post_file(&server, &format!("/api/fs/upload?dir={dir}"), "taken.txt", b"the replacement").await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "file already exists");
    assert_eq!(std::fs::read_to_string(server.home.join("taken.txt")).expect("read"), "the original");

    // A free name goes through, and the path in the response is where it landed.
    let (status, body) = post_file(&server, &format!("/api/fs/upload?dir={dir}"), "free.txt", b"brand new").await;
    assert_eq!(status, 200, "{body}");
    let saved: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(saved["sizeBytes"], 9);
    assert_eq!(saved["path"], server.home.join("free.txt").to_string_lossy().into_owned());

    // A filename is reduced to its last segment, so it cannot carry a directory.
    let (status, body) = post_file(&server, &format!("/api/fs/upload?dir={dir}"), "../../escaped.txt", b"nope").await;
    assert_eq!(status, 200, "{body}");
    let saved: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(saved["path"], server.home.join("escaped.txt").to_string_lossy().into_owned());

    // The directory has to exist and be one.
    let (status, body) = post_file(&server, &format!("/api/fs/upload?dir={dir}/taken.txt"), "x.txt", b"x").await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&body), "not a directory");

    server.stop().await;
}

#[tokio::test]
async fn a_multipart_body_without_a_file_field_fails_with_the_routes_fallback() {
    let server = TestServer::start().await;
    let body = b"--boundary\r\nContent-Disposition: form-data; name=\"other\"\r\n\r\nx\r\n--boundary--\r\n";

    let (status, response) = server.request("POST", "/api/uploads", &[("Content-Type", MULTIPART_TYPE)], body).await;
    assert_eq!(status, 400);
    assert_eq!(error_of(&response), "cannot save image");

    server.stop().await;
}

#[tokio::test]
async fn the_sweep_drops_stale_upload_directories_and_keeps_fresh_ones() {
    let server = TestServer::start().await;

    // Two uploads, then age one of them past the seven-day TTL.
    let (status, body) = post_file(&server, "/api/uploads", "old.txt", b"old").await;
    assert_eq!(status, 200, "{body}");
    let stale = Path::new(serde_json::from_str::<Value>(&body).expect("json")["path"].as_str().expect("path"))
        .parent()
        .expect("uuid directory")
        .to_path_buf();

    let (status, body) = post_file(&server, "/api/uploads", "new.txt", b"new").await;
    assert_eq!(status, 200, "{body}");
    let fresh = Path::new(serde_json::from_str::<Value>(&body).expect("json")["path"].as_str().expect("path"))
        .parent()
        .expect("uuid directory")
        .to_path_buf();

    let long_ago = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
    let handle = std::fs::File::open(&stale).expect("open directory");
    handle
        .set_times(std::fs::FileTimes::new().set_accessed(long_ago).set_modified(long_ago))
        .expect("age the directory");

    server.paths.sweep_uploads().await;

    assert!(!stale.exists(), "an upload older than the TTL should have been swept");
    assert!(fresh.exists(), "a recent upload must survive the sweep");

    server.stop().await;
}

#[tokio::test]
async fn the_spec_documents_every_filesystem_route() {
    let server = TestServer::start().await;
    let spec = server.get_json("/openapi.json").await;

    for path in ["/api/fs/list", "/api/fs/read", "/api/fs/download"] {
        let operation = &spec["paths"][path]["get"];
        assert!(operation.is_object(), "{path} is missing from the spec");
        assert!(operation["responses"]["400"].is_object(), "{path} does not document its 400");
        assert_eq!(operation["responses"]["default"], Value::Null, "the derived `default` should be replaced");
    }
    for path in ["/api/fs/upload", "/api/uploads"] {
        let operation = &spec["paths"][path]["post"];
        assert!(operation.is_object(), "{path} is missing from the spec");
        assert!(operation["requestBody"]["content"]["multipart/form-data"].is_object());
        assert!(operation["responses"]["400"].is_object(), "{path} does not document its 400");
    }

    // The download's success case is raw bytes, not the JSON its return type
    // would otherwise imply.
    let download = &spec["paths"]["/api/fs/download"]["get"]["responses"]["200"];
    assert_eq!(download["content"]["application/octet-stream"]["schema"]["format"], "binary");
    assert_eq!(download["content"]["application/json"], Value::Null);

    // `kind` is an inline enum in the zod contract, so it must not have become a
    // component of its own here.
    let kind = &spec["components"]["schemas"]["FsEntry"]["properties"]["kind"];
    assert_eq!(kind["$ref"], Value::Null, "FsKind must stay inline");
    assert_eq!(kind["enum"], serde_json::json!(["file", "dir", "symlink", "other"]));
    assert_eq!(
        spec["components"]["schemas"]["FileBytes"],
        Value::Null,
        "the download newtype is not a schema"
    );

    server.stop().await;
}

/// axum caps a request body at 2 MiB unless the route says otherwise, which node
/// had no equivalent of — so both upload routes used to refuse anything past
/// 2 MiB with a bare `413`, however large a file the API said it accepted.
#[tokio::test]
async fn an_upload_larger_than_axums_default_body_limit_is_accepted() {
    let server = TestServer::start().await;
    let dir = server.home.display().to_string();
    let content = vec![b'x'; 3 * 1024 * 1024];

    let (status, body) = post_file(&server, &format!("/api/fs/upload?dir={dir}"), "big.bin", &content).await;
    assert_eq!(status, 200, "a 3 MiB upload is far below the declared 200 MiB ceiling: {body}");
    let saved: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(saved["sizeBytes"], content.len());

    let (status, body) = post_file(&server, "/api/uploads", "big.bin", &content).await;
    assert_eq!(status, 200, "a 3 MiB attachment is far below the declared 100 MiB ceiling: {body}");
    let saved: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(saved["sizeBytes"], content.len());

    server.stop().await;
}
