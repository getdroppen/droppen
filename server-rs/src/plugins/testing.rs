//! Plugin packages for tests, built in code.
//!
//! No real plugin package is committed to this repository, so every test that
//! needs a tarball builds one here: a minimal valid package, or a deliberately
//! malicious one — a symlink, a `..` path, an absolute path, a header that
//! overstates its size. The tar is written with raw headers rather than
//! through a builder so those cases carry exactly the bytes they mean to.

use std::io::Write;

/// A `package/package.json` naming the entry `./dist/index.js`, with the
/// `droppen` manifest a package must carry. No `name`: a package that leaves
/// it out takes its id, which is the behaviour the install tests pin.
pub fn package(id: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "name": format!("@droppen/plugin-{id}"),
        "version": version,
        "type": "module",
        "exports": { ".": "./dist/index.js" },
        "droppen": {
            "id": id,
            "engines": { "droppen": "^0.1.0" },
            "permissions": ["storage"],
            "aliases": [format!("/{id}")],
        },
    })
}

/// A valid package: `package.json` plus `files`, each named inside the package
/// (`dist/index.js`).
pub fn tarball(package_json: &serde_json::Value, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut entries = vec![
        Entry::dir("package/"),
        Entry::file("package/package.json", serde_json::to_vec(package_json).expect("package.json")),
    ];
    entries.extend(files.iter().map(|(path, data)| Entry::file(format!("package/{path}"), data.to_vec())));
    raw_tarball(&entries)
}

/// One tar entry, with the header it will carry.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The raw path, `/`-separated.
    pub path: String,
    /// What the header says it is.
    pub kind: Kind,
    /// The bytes after the header.
    pub data: Vec<u8>,
    /// A size field that may disagree with `data.len()`.
    pub size: Option<u64>,
}

/// A tar header's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A regular file (`0`).
    File,
    /// A directory (`5`).
    Dir,
    /// A symbolic link (`2`), with its target in the header.
    Symlink,
    /// A hard link (`1`), with its target in the header.
    Hardlink,
}

impl Entry {
    /// A regular file.
    pub fn file(path: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            kind: Kind::File,
            data: data.into(),
            size: None,
        }
    }

    /// A directory.
    pub fn dir(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: Kind::Dir,
            data: Vec::new(),
            size: None,
        }
    }

    /// A symbolic link to `target`.
    pub fn symlink(path: impl Into<String>, target: &str) -> Self {
        Self {
            path: path.into(),
            kind: Kind::Symlink,
            data: target.as_bytes().to_vec(),
            size: Some(0),
        }
    }

    /// A hard link to `target`.
    pub fn hardlink(path: impl Into<String>, target: &str) -> Self {
        Self {
            path: path.into(),
            kind: Kind::Hardlink,
            data: target.as_bytes().to_vec(),
            size: Some(0),
        }
    }

    /// A regular file whose header claims `size` bytes, whatever `data` holds.
    pub fn file_with_size(path: impl Into<String>, size: u64, data: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            kind: Kind::File,
            data: data.into(),
            size: Some(size),
        }
    }
}

/// A gzip'd tar of exactly these entries, in order.
pub fn raw_tarball(entries: &[Entry]) -> Vec<u8> {
    let mut tar = Vec::new();
    for entry in entries {
        let link = match entry.kind {
            Kind::Symlink | Kind::Hardlink => entry.data.as_slice(),
            Kind::File | Kind::Dir => &[],
        };
        let typeflag = match entry.kind {
            Kind::File => b'0',
            Kind::Dir => b'5',
            Kind::Symlink => b'2',
            Kind::Hardlink => b'1',
        };
        let size = entry.size.unwrap_or(entry.data.len() as u64);
        tar.extend_from_slice(&header(entry.path.as_bytes(), typeflag, size, link));
        if entry.size.is_none() {
            tar.extend_from_slice(&entry.data);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
    }
    // Two zero blocks end a tar.
    tar.extend_from_slice(&[0u8; 1024]);
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&tar).expect("gz");
    gzip.finish().expect("gz")
}

/// One 512-byte ustar header. `size` is what the header says, which for a
/// lying entry is not `data.len()`.
fn header(path: &[u8], typeflag: u8, size: u64, link: &[u8]) -> [u8; 512] {
    assert!(path.len() < 100 && link.len() < 100, "the test tar writes only plain ustar names");
    let mut header = [0u8; 512];
    header[..path.len()].copy_from_slice(path);
    header[100..108].copy_from_slice(b"0000644\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    header[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].copy_from_slice(b"        ");
    header[156] = typeflag;
    header[157..157 + link.len()].copy_from_slice(link);
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    header
}
