//! Cross-chunk references: when a change is reviewed in several chunks, each
//! chunk lists where the names it defines appear in the other chunks, so a
//! model does not claim that something is unused or unwired only because its
//! callers were sent separately. Language-agnostic: definitions are found by
//! common keywords, not by a parser.

use crate::DiffChunk;
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

/// Keywords that introduce a named definition in common languages.
const DEFINITION_KEYWORDS: &[&str] = &[
    "fn",
    "def",
    "function",
    "func",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "type",
    "module",
    "record",
    "object",
    "protocol",
];
/// Shorter names are too generic to be worth listing.
const MIN_NAME_CHARS: usize = 4;
const MAX_PER_NAME: usize = 3;
const MAX_REFERENCES_PER_CHUNK: usize = 25;
const MAX_LINE_CHARS: usize = 160;

/// One line of a chunk's diff text that exists in the new file.
struct NewLine<'a> {
    path: &'a str,
    line: u32,
    text: &'a str,
}

fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| !is_identifier_char(character))
        .filter(|token| !token.is_empty())
}

/// The path of `a/<path> b/<path>`, the form of an unrenamed file. A path
/// may itself contain ` b/`, so the two halves are matched instead of
/// splitting at a separator; a rename's `+++ b/` header names it otherwise.
fn header_path(rest: &str) -> &str {
    let Some(sides) = rest.strip_prefix("a/") else {
        return "";
    };
    // `<path> b/<path>`: the path is half of what remains after ` b/`.
    let length = sides.len().saturating_sub(3) / 2;
    match (
        sides.get(..length),
        sides.get(length..length + 3),
        sides.get(length + 3..),
    ) {
        (Some(old), Some(" b/"), Some(new)) if old == new => new,
        _ => "",
    }
}

/// Lines added or kept by the chunk's hunks, with their new-file numbers.
fn new_lines(text: &str) -> Vec<NewLine<'_>> {
    let mut lines = Vec::new();
    let mut path = "";
    let mut next: Option<u32> = None;
    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            path = header_path(rest);
            next = None;
        } else if next.is_none()
            && let Some(new_path) = raw.strip_prefix("+++ b/")
        {
            // The new-file header names the path exactly; it comes before
            // the first hunk of a file.
            path = new_path;
        } else if let Some(rest) = raw.strip_prefix("@@ ") {
            next = rest
                .split_whitespace()
                .find_map(|range| range.strip_prefix('+'))
                .and_then(|range| range.split(',').next()?.parse().ok());
        } else if let Some(line) = next {
            match raw.as_bytes().first() {
                // Added and kept lines. Inside a hunk a `+++ ` line is an
                // added line whose content starts with `++ `; file headers
                // only come before a hunk.
                Some(b'+' | b' ') => {
                    lines.push(NewLine {
                        path,
                        line,
                        text: &raw[1..],
                    });
                    next = Some(line.saturating_add(1));
                }
                Some(b'-' | b'\\') => {}
                // Anything else (a Cururu note) ends the hunk.
                _ => next = None,
            }
        }
    }
    lines
}

/// Words that may precede a definition keyword at the start of a line.
const MODIFIERS: &[&str] = &[
    "pub",
    "crate",
    "super",
    "self",
    "export",
    "default",
    "async",
    "unsafe",
    "extern",
    "static",
    "public",
    "private",
    "protected",
    "internal",
    "abstract",
    "final",
    "sealed",
    "open",
    "data",
    "inline",
    "override",
    "declare",
    "const",
];

/// `pub(crate) fn x`, `pub(super) fn x` or `pub(in a::b) fn x` without the
/// `pub(...)` group, so any restricted visibility is a plain modifier.
fn without_visibility_group(text: &str) -> &str {
    text.strip_prefix("pub(")
        .and_then(|group| group.split_once(')'))
        .map_or(text, |(_, rest)| rest.trim_start())
}

/// Names defined on the chunk's new lines: a line that starts (after
/// modifiers) with a definition keyword followed by an identifier. Requiring
/// the keyword to start the line keeps strings such as `"type": "text"` out.
fn defined_names(lines: &[NewLine<'_>]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in lines {
        let text = without_visibility_group(line.text.trim_start());
        let mut tokens = identifiers(text).skip_while(|token| MODIFIERS.contains(token));
        let starts_with_keyword = text.starts_with(|character: char| is_identifier_char(character));
        if let (true, Some(keyword)) = (starts_with_keyword, tokens.next())
            && DEFINITION_KEYWORDS.contains(&keyword)
            && let Some(name) = tokens.next()
            && name.chars().count() >= MIN_NAME_CHARS
            && !DEFINITION_KEYWORDS.contains(&name)
        {
            names.insert(name.to_owned());
        }
    }
    names
}

fn excerpt(text: &str) -> String {
    let trimmed = text.trim().replace('`', "'");
    let mut chars = trimmed.chars();
    let prefix: String = chars.by_ref().take(MAX_LINE_CHARS).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// Bytes kept free in each chunk of a multi-chunk change for its
/// cross-reference note, so the annotated chunk stays within `chunk_bytes`.
#[must_use]
pub const fn cross_reference_reserve(chunk_bytes: usize) -> usize {
    let reserve = chunk_bytes / 8;
    if reserve < 4096 { reserve } else { 4096 }
}

/// Append to each chunk of a multi-chunk change a list of where the names it
/// defines appear in the other chunks, at most `max_note_bytes` long.
pub fn annotate_cross_references(chunks: &mut [DiffChunk], max_note_bytes: usize) {
    if chunks.len() < 2 {
        return;
    }
    let notes: Vec<String> = {
        let parsed: Vec<Vec<NewLine<'_>>> =
            chunks.iter().map(|chunk| new_lines(&chunk.text)).collect();
        // One pass over every line: identifier -> (chunk, line) in order.
        let mut index: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
        for (chunk_index, lines) in parsed.iter().enumerate() {
            for (line_index, line) in lines.iter().enumerate() {
                let mut seen = BTreeSet::new();
                for token in identifiers(line.text) {
                    if token.chars().count() >= MIN_NAME_CHARS && seen.insert(token) {
                        index
                            .entry(token)
                            .or_default()
                            .push((chunk_index, line_index));
                    }
                }
            }
        }
        parsed
            .iter()
            .enumerate()
            .map(|(chunk_index, lines)| {
                // Per name, uses before other definitions of the same name:
                // a call is what disproves "never called".
                let per_name: Vec<(String, Vec<&NewLine<'_>>)> = defined_names(lines)
                    .into_iter()
                    .map(|name| {
                        // One pass, stopping once enough uses are found:
                        // uses first, other definitions only to fill up.
                        let mut uses = Vec::with_capacity(MAX_PER_NAME);
                        let mut definitions = Vec::with_capacity(MAX_PER_NAME);
                        for &(other, line_index) in index.get(name.as_str()).into_iter().flatten() {
                            if other == chunk_index {
                                continue;
                            }
                            let mention = &parsed[other][line_index];
                            if defined_names(std::slice::from_ref(mention)).contains(&name) {
                                if definitions.len() < MAX_PER_NAME {
                                    definitions.push(mention);
                                }
                            } else {
                                uses.push(mention);
                                if uses.len() == MAX_PER_NAME {
                                    break;
                                }
                            }
                        }
                        uses.extend(definitions);
                        uses.truncate(MAX_PER_NAME);
                        (name, uses)
                    })
                    .collect();
                // One mention of every name first, then the further ones, so
                // a tight budget still covers as many names as possible.
                let mut entries = Vec::new();
                for round in 0..MAX_PER_NAME {
                    for (name, mentions) in &per_name {
                        if let Some(mention) = mentions.get(round) {
                            entries.push(format!(
                                "- `{name}`: {}:{} `{}`",
                                mention.path,
                                mention.line,
                                excerpt(mention.text)
                            ));
                        }
                    }
                }
                entries.truncate(MAX_REFERENCES_PER_CHUNK);
                let mut note = String::from(
                    "\n[cururu: names defined in this chunk also appear in other chunks of this change]\n",
                );
                let header = note.len();
                for entry in entries {
                    if note.len() + entry.len() + 1 > max_note_bytes {
                        break;
                    }
                    let _ = writeln!(note, "{entry}");
                }
                if note.len() == header {
                    return String::new();
                }
                note
            })
            .collect()
    };
    for (chunk, note) in chunks.iter_mut().zip(notes) {
        chunk.text.push_str(&note);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(index: usize, path: &str, body: &str) -> DiffChunk {
        DiffChunk {
            index,
            text: format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n{body}"),
            files: vec![path.into()],
        }
    }

    #[test]
    fn a_definition_lists_its_uses_in_other_chunks() {
        let mut chunks = vec![
            chunk(
                0,
                "src/engine.rs",
                "@@ -10,0 +11,3 @@\n+fn review_chunk_with_more_output(chunk: &Chunk) {\n+    retry(chunk)\n+}\n",
            ),
            chunk(
                1,
                "src/input.rs",
                "@@ -40,2 +40,3 @@\n context\n+    agent.review_chunk_with_more_output(chunk).await\n-    old_call()\n",
            ),
        ];
        annotate_cross_references(&mut chunks, 4096);
        assert!(
            chunks[0].text.ends_with(
                "[cururu: names defined in this chunk also appear in other chunks of this change]\n- `review_chunk_with_more_output`: src/input.rs:41 `agent.review_chunk_with_more_output(chunk).await`\n"
            ),
            "{}",
            chunks[0].text
        );
        // The second chunk defines nothing, so it is unchanged.
        assert!(!chunks[1].text.contains("[cururu:"));
    }

    #[test]
    fn single_chunks_removed_lines_and_short_names_add_nothing() {
        let mut single = vec![chunk(0, "a.py", "@@ -0,0 +1 @@\n+def load_config():\n")];
        annotate_cross_references(&mut single, 4096);
        assert!(!single[0].text.contains("[cururu:"));

        let mut chunks = vec![
            chunk(
                0,
                "a.py",
                "@@ -0,0 +1,2 @@\n+def load_config():\n+def run():\n",
            ),
            // Only a removed line and a short name mention the definitions.
            chunk(1, "b.py", "@@ -1 +1 @@\n-load_config()\n+run()\n"),
        ];
        annotate_cross_references(&mut chunks, 4096);
        assert!(!chunks[0].text.contains("[cururu:"), "{}", chunks[0].text);
    }

    #[test]
    fn keywords_inside_expressions_or_strings_are_not_definitions() {
        let mut chunks = vec![
            chunk(
                0,
                "a.rs",
                "@@ -0,0 +1,5 @@\n+let value = json!({\"type\": \"text\"});\n+    pub(crate) async fn load_config() {}\n+export default class Widget {}\n+pub(super) fn save_config() {}\n+impl Display for Report {}\n",
            ),
            chunk(
                1,
                "b.rs",
                "@@ -0,0 +1 @@\n+use(text, load_config, Widget, save_config, Display);\n",
            ),
        ];
        annotate_cross_references(&mut chunks, 4096);
        let note = &chunks[0].text;
        assert!(note.contains("- `load_config`: b.rs:1"), "{note}");
        assert!(note.contains("- `Widget`: b.rs:1"), "{note}");
        assert!(note.contains("- `save_config`: b.rs:1"), "{note}");
        assert!(!note.contains("- `text`"), "{note}");
        // `impl Display for Report` uses `Display`; it does not define it.
        assert!(!note.contains("- `Display`"), "{note}");
    }

    #[test]
    fn added_lines_starting_with_plus_signs_keep_line_numbers() {
        let mut chunks = vec![
            chunk(0, "a.py", "@@ -0,0 +1 @@\n+def load_config():\n"),
            chunk(1, "b.md", "@@ -0,0 +1,2 @@\n+++ heading\n+load_config()\n"),
        ];
        annotate_cross_references(&mut chunks, 4096);
        assert!(
            chunks[0].text.contains("- `load_config`: b.md:2"),
            "{}",
            chunks[0].text
        );
    }

    #[test]
    fn paths_with_b_separators_and_restricted_visibility_are_handled() {
        assert_eq!(header_path("a/foo b/bar.rs b/foo b/bar.rs"), "foo b/bar.rs");
        assert_eq!(header_path("a/old.rs b/new.rs"), "");
        let mut chunks = vec![
            chunk(
                0,
                "src/x b/lib.rs",
                "@@ -0,0 +1 @@\n+pub(in crate::app) fn load_config() {}\n",
            ),
            chunk(1, "src/y b/use.rs", "@@ -0,0 +1 @@\n+load_config();\n"),
        ];
        annotate_cross_references(&mut chunks, 4096);
        assert!(
            chunks[0].text.contains("- `load_config`: src/y b/use.rs:1"),
            "{}",
            chunks[0].text
        );
    }

    #[test]
    fn the_note_fits_its_byte_budget() {
        let mut chunks = vec![
            chunk(
                0,
                "a.py",
                "@@ -0,0 +1,2 @@\n+def load_config():\n+def save_config():\n",
            ),
            chunk(
                1,
                "b.py",
                "@@ -0,0 +1,2 @@\n+load_config()\n+save_config()\n",
            ),
        ];
        let before = chunks[0].text.len();
        annotate_cross_references(&mut chunks, 140);
        let note = &chunks[0].text[before..];
        assert!(note.len() <= 140, "{} bytes: {note}", note.len());
        assert!(
            note.contains("load_config") && !note.contains("save_config"),
            "{note}"
        );
        let mut tiny = vec![
            chunk(0, "a.py", "@@ -0,0 +1 @@\n+def load_config():\n"),
            chunk(1, "b.py", "@@ -0,0 +1 @@\n+load_config()\n"),
        ];
        annotate_cross_references(&mut tiny, 10);
        assert!(
            !tiny[0].text.contains("[cururu:"),
            "a note without entries is left out"
        );
        assert_eq!(cross_reference_reserve(45_000), 4096);
        assert_eq!(cross_reference_reserve(8_000), 1000);
    }

    #[test]
    fn a_tight_budget_lists_calls_of_every_name_first() {
        let mut chunks = vec![
            chunk(
                0,
                "a.rs",
                "@@ -0,0 +1,2 @@\n+fn alpha_handler() {}\n+fn zulu_handler() {}\n",
            ),
            chunk(
                1,
                "b.rs",
                "@@ -0,0 +1,4 @@\n+fn alpha_handler() {}\n+alpha_handler();\n+alpha_handler();\n+zulu_handler();\n",
            ),
        ];
        let before = chunks[0].text.len();
        annotate_cross_references(&mut chunks, 200);
        let note = &chunks[0].text[before..];
        // Both names fit, each with its call rather than the other definition.
        assert!(note.contains("- `alpha_handler`: b.rs:2"), "{note}");
        assert!(note.contains("- `zulu_handler`: b.rs:4"), "{note}");
    }

    #[test]
    fn references_are_bounded() {
        let mut body = String::from("@@ -0,0 +1,40 @@\n");
        for n in 0..40 {
            let _ = writeln!(body, "+function handler{n:02}() {{}}");
        }
        let mut uses = String::from("@@ -0,0 +1,40 @@\n");
        for n in 0..40 {
            let _ = writeln!(
                uses,
                "+handler{n:02}(); handler{n:02}(); // {}",
                "x".repeat(300)
            );
        }
        let mut chunks = vec![chunk(0, "a.js", &body), chunk(1, "b.js", &uses)];
        annotate_cross_references(&mut chunks, usize::MAX);
        let entries = chunks[0]
            .text
            .lines()
            .filter(|line| line.starts_with("- `handler"))
            .count();
        assert_eq!(entries, MAX_REFERENCES_PER_CHUNK);
        assert!(
            chunks[0]
                .text
                .lines()
                .all(|line| line.chars().count() < MAX_LINE_CHARS + 60)
        );
    }
}
