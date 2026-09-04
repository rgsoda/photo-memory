//! References from one note to another.
//!
//! Two kinds, and the difference is the point of M4. A plain `[[name]]` is a
//! mention. A `supersedes:` line is a claim that the note it names has been
//! corrected, which the viewer turns into a banner — so a supersession read as
//! an ordinary link would go quiet in exactly the case the banner exists to
//! catch.

/// Prefix of a body line that declares a supersession.
///
/// The picker writes this into the body rather than the frontmatter, because it
/// is inserted at a cursor and the frontmatter is machine-managed. `Note` reads
/// the frontmatter form DESIGN.md §2 documents as well, so a note written by
/// hand is understood too.
const SUPERSEDES: &str = "supersedes:";

/// Every `[[name]]` in `text`, in order and without duplicates.
///
/// `![[name]]` is an attachment embed, so a bare search for `[[` would collect
/// every pasted screenshot as a link to a note that does not exist.
pub(crate) fn targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, _) in text.match_indices("[[") {
        if text[..i].ends_with('!') {
            continue;
        }
        let rest = &text[i + 2..];
        let Some(end) = rest.find("]]") else { continue };
        // Links are written the way the filename reads, and our own copy-link
        // affordance hands out the `.md` form, so accept both.
        let name = rest[..end].trim().trim_end_matches(".md");
        // A link names one file: it cannot nest or span lines.
        if name.is_empty() || name.contains(['[', ']', '\n']) {
            continue;
        }
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// Notes this body mentions with `[[name]]`.
///
/// Targets on a `supersedes:` line are left out — they are the other kind.
pub fn links(body: &str) -> Vec<String> {
    collect(body, false)
}

/// Notes this body declares itself to supersede.
pub fn supersedes(body: &str) -> Vec<String> {
    collect(body, true)
}

/// Walk the body one line at a time, keeping only lines of the wanted kind.
/// Supersession is a claim about the note as a whole, so it owns its line.
fn collect(body: &str, typed: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in body.lines() {
        if line.trim_start().starts_with(SUPERSEDES) != typed {
            continue;
        }
        for name in targets(line) {
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_in_order_without_duplicates() {
        let body = "Title\n\nSee [[a-note]] and [[b-note]].\nAgain [[a-note]].";
        assert_eq!(links(body), vec!["a-note", "b-note"]);
    }

    #[test]
    fn an_embed_is_an_attachment_not_a_link() {
        // Every pasted screenshot would otherwise become a dangling link.
        assert!(links("![[2026-09-04-e382057b79ca.webp]]").is_empty());
        assert_eq!(links("![[shot.webp]] but see [[a-note]]"), vec!["a-note"]);
    }

    #[test]
    fn supersedes_is_a_separate_kind() {
        let body = "New finding\nsupersedes: [[old-note]]\nRelated to [[other]].";
        assert_eq!(supersedes(body), vec!["old-note"]);
        // The superseded note must not also arrive as an ordinary mention, or
        // the banner and the backlink list would both claim it.
        assert_eq!(links(body), vec!["other"]);
    }

    #[test]
    fn a_note_with_no_references_has_none() {
        assert!(links("Just a title\n\nsome prose").is_empty());
        assert!(supersedes("Just a title").is_empty());
    }

    #[test]
    fn accepts_the_dot_md_form_our_own_copy_link_produces() {
        assert_eq!(links("see [[a-note.md]]"), vec!["a-note"]);
    }

    #[test]
    fn ignores_malformed_references() {
        assert!(links("[[never closed").is_empty());
        assert!(links("[[]]").is_empty());
        assert!(links("[[ ]]").is_empty());
    }

    #[test]
    fn links_in_the_title_line_count() {
        // The title is part of the body, and a title may well name another note.
        assert_eq!(links("Follow-up to [[a-note]]\n\nbody"), vec!["a-note"]);
    }
}
