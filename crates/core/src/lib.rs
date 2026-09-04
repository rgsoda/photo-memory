//! Pure note format: parsing, rendering, titles, slugs, filenames.
//!
//! No I/O lives here. String in, struct out, string back.

mod frontmatter;
mod links;
mod tags;
mod note;
mod slug;

pub use frontmatter::Frontmatter;
pub use links::{embeds, links, supersedes};
pub use tags::tags;
pub use note::{Note, ParseError};
pub use slug::slugify;
