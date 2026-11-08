//! What a tool-less claude instant chat can take from an upload.
//!
//! A session hands claude an attachment path and the Read tool opens it. An
//! instant chat has no tools at all (`--disallowedTools *`): Read there would
//! be bounded by the user's own CLI settings — an allow rule, `defaultMode` —
//! rather than by this server's uploads directory, so it must not be offered.
//! Instead:
//!
//! * **images** are read here, at send and again at delivery, and become
//!   base64 image content blocks in the stream-json user line;
//! * **non-image files** are read here and inlined into the prompt as UTF-8
//!   text, with a delimiter naming the file (see
//!   [`droppen_sessions::util::chat_file_block`]).
//!
//! Everything is re-fenced when it is read: a path is accepted only when its
//! lexical spelling and its canonical target are both inside
//! [`Paths::uploads_dir`], and the handle is then opened `O_NOFOLLOW |
//! O_NONBLOCK` on a blocking thread, proven to be a regular file with one
//! link, and read through `take(cap + 1)` — so a symlink, FIFO, hardlink or a
//! file that grew after the check is refused rather than followed, hung on or
//! read whole. The send routes run the same reads to refuse a bad attachment
//! with a `400`; the driver runs them again at delivery so a queue that
//! outlived the upload fails loudly rather than sending an empty prompt.

use std::path::{Component, Path, PathBuf};

use base64::Engine as _;

use droppen_instance::paths::Paths;

/// The largest image a claude chat sends, in bytes after reading.
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
/// The largest total of image bytes one message carries.
///
/// Twenty megabytes of image bytes become about 26.7 MB of base64 in a single
/// stdin line; with the text and JSON framing that stays under the CLI's 32 MB
/// request limit with headroom. Larger messages are refused rather than built.
pub const MAX_IMAGE_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
/// The largest single text file a claude chat inlines.
pub const MAX_TEXT_FILE_BYTES: u64 = 100 * 1024;
/// The largest total of inlined text one message carries.
pub const MAX_TEXT_TOTAL_BYTES: u64 = 200 * 1024;

/// An image ready to travel as an `image` content block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatImage {
    /// From the bytes: `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub media_type: &'static str,
    /// The whole file, standard base64.
    pub data: String,
    /// The file's size before base64, for the per-message total.
    pub raw_len: u64,
}

/// Why a chat attachment could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatAttachmentError {
    /// Not a path from `/api/uploads` (or a symlink leading out of it).
    OutsideUploads,
    /// Gone from the device since the upload, or not a regular file.
    Missing,
    /// Over [`MAX_IMAGE_BYTES`].
    ImageTooLarge {
        /// The upload's file name.
        name: String,
        /// The cap that was passed.
        limit: u64,
    },
    /// Over [`MAX_TEXT_FILE_BYTES`].
    FileTooLarge {
        /// The upload's file name.
        name: String,
        /// The cap that was passed.
        limit: u64,
    },
    /// The message's text files together are over [`MAX_TEXT_TOTAL_BYTES`].
    TextTooLarge {
        /// The cap that was passed.
        limit: u64,
    },
    /// The message's images together are over [`MAX_IMAGE_TOTAL_BYTES`].
    ImagesTooLarge {
        /// The cap that was passed.
        limit: u64,
    },
    /// A file with more than one link: a hardlink made inside uploads could
    /// otherwise point at a file that is not an upload at all.
    HardLinked {
        /// The upload's file name.
        name: String,
    },
    /// Not one of the four image types the API takes.
    UnsupportedImage {
        /// The upload's file name.
        name: String,
    },
    /// Not UTF-8.
    NotText {
        /// The upload's file name.
        name: String,
    },
}

impl std::fmt::Display for ChatAttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideUploads => f.write_str("attachment paths must come from /api/uploads"),
            Self::Missing => f.write_str("an attachment is no longer on this device; attach it again"),
            Self::ImageTooLarge { name, limit } => {
                write!(f, "{name} is too large for a claude chat (images are at most {} MB)", limit / (1024 * 1024))
            }
            Self::FileTooLarge { name, limit } => {
                write!(f, "{name} is too large for a claude chat (text files are at most {} KB)", limit / 1024)
            }
            Self::TextTooLarge { limit } => {
                write!(f, "the attached text files are too large together for a claude chat (at most {} KB)", limit / 1024)
            }
            Self::ImagesTooLarge { limit } => {
                write!(f, "the attached images are too large together for a claude chat (at most {} MB)", limit / (1024 * 1024))
            }
            Self::HardLinked { name } => write!(f, "{name} is hard-linked to a file outside the uploads directory"),
            Self::UnsupportedImage { name } => {
                write!(f, "{name} is not an image a claude chat can take (PNG, JPEG, GIF or WebP)")
            }
            Self::NotText { name } => write!(f, "{name} is not a UTF-8 text file a claude chat can take"),
        }
    }
}

impl std::error::Error for ChatAttachmentError {}

/// Decode the base64 payload of an `image` content block.
pub fn image_bytes(data: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

/// The media type of `bytes`, when it is one the API takes.
pub fn image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Resolve `requested` to the canonical file it names, or explain why it is
/// not an upload.
///
/// The lexical spelling is normalised first and must be under the uploads
/// directory (the uploads directory itself is not a file). The canonical
/// target is then checked separately, so a symlink planted inside uploads
/// cannot lead out of it. This mirrors `droppen_files::FileRoots`'s fence
/// without depending on the files crate; the canonical path is what callers
/// read.
pub fn fenced_upload(paths: &Paths, requested: &str) -> Result<PathBuf, ChatAttachmentError> {
    let uploads = paths.uploads_dir();
    let requested = Path::new(requested);
    if !requested.is_absolute() {
        return Err(ChatAttachmentError::OutsideUploads);
    }
    let mut normalised = PathBuf::from("/");
    for component in requested.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => normalised.push(part),
            Component::ParentDir => {
                if !normalised.pop() {
                    return Err(ChatAttachmentError::OutsideUploads);
                }
            }
            Component::Prefix(_) => return Err(ChatAttachmentError::OutsideUploads),
        }
    }
    if normalised == uploads || !normalised.starts_with(&uploads) {
        return Err(ChatAttachmentError::OutsideUploads);
    }
    let real = std::fs::canonicalize(&normalised).map_err(|_| ChatAttachmentError::Missing)?;
    let uploads_real =
        std::fs::canonicalize(&uploads).map_err(|_| ChatAttachmentError::Missing)?;
    if real == uploads_real || !real.starts_with(&uploads_real) {
        return Err(ChatAttachmentError::OutsideUploads);
    }
    if !std::fs::metadata(&real).is_ok_and(|meta| meta.is_file()) {
        return Err(ChatAttachmentError::Missing);
    }
    Ok(real)
}

/// The file name an error names, from the path the client sent.
fn name_of(requested: &str) -> String {
    Path::new(requested)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| requested.to_string())
}

/// Which cap a read is under.
#[derive(Debug, Clone, Copy)]
enum Cap {
    /// [`MAX_IMAGE_BYTES`].
    Image,
    /// [`MAX_TEXT_FILE_BYTES`].
    File,
}

impl Cap {
    fn limit(self) -> u64 {
        match self {
            Self::Image => MAX_IMAGE_BYTES,
            Self::File => MAX_TEXT_FILE_BYTES,
        }
    }

    fn too_large(self, requested: &str) -> ChatAttachmentError {
        match self {
            Self::Image => ChatAttachmentError::ImageTooLarge {
                name: name_of(requested),
                limit: MAX_IMAGE_BYTES,
            },
            Self::File => ChatAttachmentError::FileTooLarge {
                name: name_of(requested),
                limit: MAX_TEXT_FILE_BYTES,
            },
        }
    }
}

/// Open and read one fenced upload, off the async runtime.
///
/// The path is re-fenced first. The open itself is what closes the race the
/// fence leaves: `O_NOFOLLOW` refuses a final component swapped for a symlink,
/// `O_NONBLOCK` means a FIFO cannot hang the driver, `fstat` on that very
/// handle proves it is a regular, singly-linked file under the cap, and the
/// read is through `take(cap + 1)` so a file that grows after the check is cut
/// off instead of read whole.
async fn read_fenced(paths: Paths, requested: String, cap: Cap) -> Result<Vec<u8>, ChatAttachmentError> {
    let limit = cap.limit();
    let job = move || -> Result<Vec<u8>, ChatAttachmentError> {
        let path = fenced_upload(&paths, &requested)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        }
        let mut file = options.open(&path).map_err(|_| ChatAttachmentError::Missing)?;
        let meta = file.metadata().map_err(|_| ChatAttachmentError::Missing)?;
        if !meta.is_file() {
            return Err(ChatAttachmentError::Missing);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                return Err(ChatAttachmentError::HardLinked { name: name_of(&requested) });
            }
        }
        if meta.len() > limit {
            return Err(cap.too_large(&requested));
        }
        let mut bytes = Vec::new();
        use std::io::Read as _;
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ChatAttachmentError::Missing)?;
        if bytes.len() as u64 > limit {
            return Err(cap.too_large(&requested));
        }
        Ok(bytes)
    };
    match tokio::task::spawn_blocking(job).await {
        Ok(result) => result,
        Err(_) => Err(ChatAttachmentError::Missing),
    }
}

/// Read one image from uploads, ready to become an `image` content block.
pub async fn read_chat_image(paths: &Paths, requested: &str) -> Result<ChatImage, ChatAttachmentError> {
    let bytes = read_fenced(paths.clone(), requested.to_string(), Cap::Image).await?;
    let media_type = image_media_type(&bytes).ok_or_else(|| ChatAttachmentError::UnsupportedImage {
        name: name_of(requested),
    })?;
    let raw_len = bytes.len() as u64;
    Ok(ChatImage {
        media_type,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        raw_len,
    })
}

/// Read one non-image file from uploads as UTF-8 text, adding its size to
/// `total` for the per-message cap.
pub async fn read_chat_text_file(
    paths: &Paths,
    requested: &str,
    total: &mut u64,
) -> Result<String, ChatAttachmentError> {
    let bytes = read_fenced(paths.clone(), requested.to_string(), Cap::File).await?;
    *total = total.saturating_add(bytes.len() as u64);
    if *total > MAX_TEXT_TOTAL_BYTES {
        return Err(ChatAttachmentError::TextTooLarge {
            limit: MAX_TEXT_TOTAL_BYTES,
        });
    }
    String::from_utf8(bytes).map_err(|_| ChatAttachmentError::NotText {
        name: name_of(requested),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(home: &Path) -> Paths {
        Paths::new(home.to_path_buf())
    }

    fn upload(paths: &Paths, name: &str, bytes: &[u8]) -> String {
        let dir = paths.uploads_dir().join("abc");
        std::fs::create_dir_all(&dir).expect("uploads dir");
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("upload");
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn only_the_four_api_image_types_are_recognised() {
        assert_eq!(image_media_type(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0]), Some("image/png"));
        assert_eq!(image_media_type(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(image_media_type(b"GIF89a...."), Some("image/gif"));
        assert_eq!(image_media_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_media_type(b"BM...."), None);
        assert_eq!(image_media_type(b"<svg"), None);
    }

    #[tokio::test]
    async fn an_image_is_read_fenced_and_base64_encoded() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
        let path = upload(&paths, "a.png", &png);
        let image = read_chat_image(&paths, &path).await.expect("an upload");
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.raw_len, png.len() as u64);
        assert_eq!(
            base64::engine::general_purpose::STANDARD.decode(&image.data).expect("base64"),
            png
        );
    }

    /// A delivery re-reads an upload that may have been swept since the send;
    /// it is refused with the reason the composer shows, not read as empty.
    #[tokio::test]
    async fn a_swept_upload_is_refused_at_delivery() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        std::fs::create_dir_all(paths.uploads_dir().join("abc")).expect("uploads dir");
        let gone = paths.uploads_dir().join("abc/gone.txt").to_string_lossy().into_owned();
        let error = read_chat_text_file(&paths, &gone, &mut 0).await.expect_err("swept");
        assert_eq!(error, ChatAttachmentError::Missing);
        let gone_image = paths.uploads_dir().join("abc/gone.png").to_string_lossy().into_owned();
        let error = read_chat_image(&paths, &gone_image).await.expect_err("swept");
        assert_eq!(error, ChatAttachmentError::Missing);
    }

    /// A FIFO swapped in after the send must not hang the driver: the open is
    /// non-blocking and the handle is proven to be a regular file.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_fifo_is_refused_without_blocking() {
        use std::ffi::CString;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let fifo = paths.uploads_dir().join("abc/fifo.txt");
        std::fs::create_dir_all(fifo.parent().expect("parent")).expect("uploads dir");
        let raw = CString::new(fifo.to_string_lossy().as_bytes()).expect("path");
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0, "mkfifo");
        let error = read_chat_text_file(&paths, &fifo.to_string_lossy(), &mut 0).await.expect_err("fifo");
        assert_eq!(error, ChatAttachmentError::Missing);
    }

    /// A hardlink made inside uploads could name a file that is not an upload;
    /// it is refused by the link count of the opened handle.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_hardlinked_upload_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let first = upload(&paths, "a.txt", b"hello");
        let second = paths.uploads_dir().join("abc/b.txt");
        std::fs::hard_link(&first, &second).expect("hardlink");
        let error = read_chat_text_file(&paths, &second.to_string_lossy(), &mut 0).await.expect_err("hardlink");
        assert!(matches!(error, ChatAttachmentError::HardLinked { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn a_path_outside_uploads_is_refused_even_when_it_exists() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let secrets = home.path().join("id_rsa");
        std::fs::write(&secrets, b"secret").expect("file");
        let error = read_chat_text_file(&paths, &secrets.to_string_lossy(), &mut 0).await.expect_err("outside");
        assert_eq!(error, ChatAttachmentError::OutsideUploads);
        let escape = paths.uploads_dir().join("abc/../../../id_rsa");
        let error = read_chat_text_file(&paths, &escape.to_string_lossy(), &mut 0).await.expect_err("escape");
        assert_eq!(error, ChatAttachmentError::OutsideUploads);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_out_of_uploads_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let target = home.path().join("outside.txt");
        std::fs::write(&target, b"outside").expect("target");
        let link = paths.uploads_dir().join("abc/link.txt");
        std::fs::create_dir_all(link.parent().expect("parent")).expect("uploads dir");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let error = read_chat_text_file(&paths, &link.to_string_lossy(), &mut 0).await.expect_err("symlink");
        assert_eq!(error, ChatAttachmentError::OutsideUploads);
    }

    #[tokio::test]
    async fn the_caps_and_the_utf8_test_are_enforced() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let big_image = upload(&paths, "big.png", &vec![0u8; (MAX_IMAGE_BYTES + 1) as usize]);
        let error = read_chat_image(&paths, &big_image).await.expect_err("too large");
        assert!(matches!(error, ChatAttachmentError::ImageTooLarge { .. }));

        let big_file = upload(&paths, "big.txt", &vec![b'a'; (MAX_TEXT_FILE_BYTES + 1) as usize]);
        let error = read_chat_text_file(&paths, &big_file, &mut 0).await.expect_err("too large");
        assert!(matches!(error, ChatAttachmentError::FileTooLarge { .. }));

        let binary = upload(&paths, "binary.bin", &[0xff, 0xfe, 0x00]);
        let error = read_chat_text_file(&paths, &binary, &mut 0).await.expect_err("not text");
        assert!(matches!(error, ChatAttachmentError::NotText { .. }));

        let text = upload(&paths, "notes.txt", b"hello");
        let mut total = 0;
        assert_eq!(read_chat_text_file(&paths, &text, &mut total).await.expect("text"), "hello");
    }
}
