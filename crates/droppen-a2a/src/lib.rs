//! Droppen's A2A v1.0 server (`docs/a2a.md`): other agents hand this device a
//! task, follow it, answer its questions and take its result.

pub mod backend;
/// A fake [`backend::SessionBackend`] for tests. Only built for tests and with
/// the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
