//! The upload fence a client's attachments pass before a session sees them.

use droppen_http::error::{ApiError, Hinted};
use droppen_instance::paths::Paths;
use gotcha::axum::http::StatusCode;

/// The `error` of a `409` for an attachment that is not there any more.
pub(crate) const ATTACHMENT_MISSING: &str = "attachment_missing";

/// How many attachments one message carries at most.
///
/// The composer's own cap (`MAX_ATTACHMENTS` in `composer-box.tsx`, 20), made
/// the server's too: the two message routes and the two chat routes all take
/// the field, and a client that bypasses the composer must be refused in the
/// same words rather than hand a CLI a prompt with a hundred paths in it.
pub(crate) const MAX_ATTACHMENTS: usize = 20;

/// The refusal of a message carrying more than [`MAX_ATTACHMENTS`].
fn too_many() -> ApiError {
    ApiError::bad_request(format!("too many attachments (at most {MAX_ATTACHMENTS})"))
}

/// The refusal of an attachment that `/api/uploads` handed out and that is no
/// longer on this device: swept with the rest of the old uploads, or deleted.
fn attachment_missing() -> ApiError {
    ApiError::refusal(
        StatusCode::CONFLICT,
        Hinted {
            error: ATTACHMENT_MISSING,
            hint: "an attachment is no longer on this device; attach it again",
        },
    )
}

/// Refuse what a tool-less claude chat cannot take, before anything is
/// accepted.
///
/// A chat's images become base64 content blocks and its non-image files are
/// inlined as UTF-8 text, so both are read here and the same reads run again
/// in the driver at delivery. This is where a binary file, an image that is
/// not PNG/JPEG/GIF/WebP, or anything over the caps becomes a `400` the client
/// can show, rather than a turn that fails after the send was answered.
pub(crate) async fn validate_claude_chat_attachments(
    paths: &Paths,
    images: &[String],
    files: &[String],
) -> Result<(), ApiError> {
    let mut image_total = 0u64;
    for path in images {
        let image = droppen_provider::chat_attachments::read_chat_image(paths, path)
            .await
            .map_err(chat_attachment_refusal)?;
        image_total = image_total.saturating_add(image.raw_len);
        if image_total > droppen_provider::chat_attachments::MAX_IMAGE_TOTAL_BYTES {
            return Err(ApiError::bad_request(
                droppen_provider::chat_attachments::ChatAttachmentError::ImagesTooLarge {
                    limit: droppen_provider::chat_attachments::MAX_IMAGE_TOTAL_BYTES,
                }
                .to_string(),
            ));
        }
    }
    let mut total = 0u64;
    for path in files {
        droppen_provider::chat_attachments::read_chat_text_file(paths, path, &mut total)
            .await
            .map_err(chat_attachment_refusal)?;
    }
    Ok(())
}

/// A read failure at the status the message routes give it: the fence's own
/// `409`s for a path that is not an upload or is gone, and a `400` for what the
/// chat cannot carry.
fn chat_attachment_refusal(error: droppen_provider::chat_attachments::ChatAttachmentError) -> ApiError {
    use droppen_provider::chat_attachments::ChatAttachmentError as Refused;
    match error {
        Refused::OutsideUploads => ApiError::conflict("attachment paths must come from /api/uploads"),
        Refused::Missing => attachment_missing(),
        other => ApiError::bad_request(other.to_string()),
    }
}

/// Prove every attachment came from `/api/uploads` and is still there, and
/// return the resolved paths.
///
/// The gate is `isUploadPath` in the former TypeScript server's `app.ts`. The
/// start and send routes run it before a provider sees the paths, because a
/// provider hands them to a child process (`-i`, `--add-dir`, the prompt
/// suffix): a path that escaped the uploads directory would widen what the CLI
/// may read. The
/// *resolved* paths are returned because [`droppen_files::FileRoots::assert_under_uploads`]
/// only vouches for what it resolved, not for the spelling the client sent.
///
/// The lexical check comes first and keeps its own refusal. Then the file
/// itself: it must exist, be a regular file, and — symlinks followed — still be
/// inside the uploads directory. One that is gone (a draft kept across the
/// sweep of old uploads) is refused with [`ATTACHMENT_MISSING`] rather than
/// handed to an agent that would find nothing at the path.
pub(crate) fn resolve_attachments(paths: &Paths, images: Vec<String>, files: Vec<String>) -> Result<(Vec<String>, Vec<String>), ApiError> {
    if images.is_empty() && files.is_empty() {
        return Ok((images, files));
    }
    if images.len() + files.len() > MAX_ATTACHMENTS {
        return Err(too_many());
    }
    let uploads = std::fs::canonicalize(paths.uploads_dir()).ok();
    let roots = droppen_files::FileRoots::of(paths);
    let resolve = |requested: Vec<String>| -> Result<Vec<String>, ApiError> {
        requested
            .into_iter()
            .map(|path| {
                let resolved = roots.assert_under_uploads(std::path::Path::new(&path))?;
                let real = std::fs::canonicalize(&resolved).map_err(|_| attachment_missing())?;
                let inside = uploads.as_ref().is_some_and(|uploads| real != *uploads && real.starts_with(uploads));
                if !inside {
                    return Err(ApiError::new(StatusCode::CONFLICT, "attachment paths must come from /api/uploads"));
                }
                if !std::fs::metadata(&real).is_ok_and(|meta| meta.is_file()) {
                    return Err(attachment_missing());
                }
                Ok(resolved.to_string_lossy().into_owned())
            })
            .collect()
    };
    Ok((resolve(images)?, resolve(files)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cap counts images and files together, and is checked before any
    /// path is resolved: a client cannot learn which of its paths exist by
    /// sending one too many.
    #[test]
    fn more_attachments_than_the_composer_allows_are_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let uploads = paths.uploads_dir();
        std::fs::create_dir_all(uploads.join("abc")).expect("uploads dir");
        std::fs::write(uploads.join("abc/a.png"), b"x").expect("upload");
        std::fs::write(uploads.join("abc/a.txt"), b"x").expect("upload");
        let image = uploads.join("abc/a.png").to_string_lossy().into_owned();
        let file = uploads.join("abc/a.txt").to_string_lossy().into_owned();

        // Twenty, the composer's own cap, is fine — images and files together.
        let at_cap = resolve_attachments(&paths, vec![image.clone(); 18], vec![file.clone(); 2]).expect("twenty attachments");
        assert_eq!((at_cap.0.len(), at_cap.1.len()), (18, 2));

        // One more is refused before any path is resolved: nothing can be
        // learned about a path by sending a body that is over the cap.
        let refused = resolve_attachments(&paths, vec![image; 19], vec![file; 2]).expect_err("twenty-one attachments");
        assert_eq!(refused.status(), gotcha::axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(refused.body()["error"], "too many attachments (at most 20)");
        let unknown = resolve_attachments(&paths, vec!["/etc/passwd".to_string(); 21], vec![]).expect_err("over the cap");
        assert_eq!(unknown.body()["error"], "too many attachments (at most 20)");
    }

    /// The refusal, byte for byte: clients act on `error` and show `hint`.
    #[test]
    fn a_missing_attachment_is_refused_as_pinned() {
        let refused = attachment_missing();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(
            refused.body().to_string(),
            r#"{"error":"attachment_missing","hint":"an attachment is no longer on this device; attach it again"}"#
        );
    }
}
