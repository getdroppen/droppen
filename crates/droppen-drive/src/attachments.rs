//! The upload fence a client's attachments pass before a session sees them.

use droppen_http::error::{ApiError, Hinted};
use droppen_instance::paths::Paths;
use gotcha::axum::http::StatusCode;

/// The `error` of a `409` for an attachment that is not there any more.
pub(crate) const ATTACHMENT_MISSING: &str = "attachment_missing";

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
