//! What the server's tests reach of the drive that production code does not:
//! two of the routes' helpers, for the tests that read a session the providers
//! the entry registers wrote, and the list feed's broadcast capacity, for the
//! test of a list client that never reads. Only built for tests and with the
//! `test-support` feature.

use droppen_http::error::ApiError;
use droppen_sessions::attention::FirstUnread;
use droppen_sessions::Source;

use crate::feed::search::SessionSearchResult;
use crate::routes::SearchQuery;
use crate::Drive;

/// How many frames the list feed's broadcast holds: a test that fills it
/// hands a client that never reads every one.
pub const LIST_CHANNEL_CAPACITY: usize = crate::feed::global::CHANNEL_CAPACITY;

/// Search session `id` of `source` as `GET /api/sessions/{source}/{id}/search`
/// does, in the feed's numbering.
pub async fn search_session(drive: &Drive, source: Source, id: &str, query: &SearchQuery) -> Result<SessionSearchResult, ApiError> {
    crate::routes::search_session(drive, source, id, query).await
}

/// The first item of session `id` of `source` after `after`, as the session
/// detail's unread marker is found.
pub async fn first_unread_of(drive: &Drive, source: Source, id: &str, after: Option<&str>) -> Result<FirstUnread, ApiError> {
    crate::routes::first_unread_of(drive, source, id, after).await
}
