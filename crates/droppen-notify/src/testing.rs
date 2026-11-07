//! What the server's tests reach of notify beyond what the server itself
//! calls: the options a parsed `notify` command line asks for, which its tests
//! check after parsing it through the server's command tree
//! (`cli::tests::notify`). Only with `test-support`, which only the server's
//! `[dev-dependencies]` turn on; none of it is in a normal or release build.
//!
//! Each method is notify's own, under the same name.

use crate::notify::cli::Options;

/// A parsed `notify` command line, as the server's tests read it.
pub trait NotifyArgsTesting {
    /// The options this command line asks for, or the refusal.
    fn options(&self) -> Result<Options, String>;
}
