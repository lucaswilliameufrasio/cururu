//! Cross-chunk references: when a change is reviewed in several chunks, each
//! chunk lists where the names it defines appear in the other chunks, so a
//! model does not claim that something is unused or unwired only because its
//! callers were sent separately. Language-agnostic: definitions are found by
//! common keywords, not by a parser.

use crate::DiffChunk;
use std::collections::BTreeSet;
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
    "impl",
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

/// Lines added or kept by the chunk's hunks, with their new-file numbers.
fn new_lines(text: &str) -> Vec<NewLine<'_>> {
    let mut lines = Vec::new();
    let mut path = "";
    let mut next: Option<u32> = None;
    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            path = rest.rsplit_once(" b/").map_or("", |(_, path)| path);
            next = None;
        } else if let Some(rest) = raw.strip_prefix("@@ ") {
            next = rest
                .split_whitespace()
                .find_map(|range| range.strip_prefix('+'))
                .and_then(|range| range.split(',').next()?.parse().ok());
        } else if let Some(line) = next {
            match raw.as_bytes().first() {
                Some(b'+') if !raw.starts_with("+++ ") => {
                    lines.push(NewLine {
                        path,
                        line,
                        text: &raw[1..],
                    });
                    next = Some(line.saturating_add(1));
                }
                Some(b' ') => {
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

/// Names defined on the chunk's new lines: a line that starts (after
/// modifiers) with a definition keyword followed by an identifier. Requiring
/// the keyword to start the line keeps strings such as `"type": "text"` out.
fn defined_names(lines: &[NewLine<'_>]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in lines {
        let mut tokens = identifiers(line.text).skip_while(|token| MODIFIERS.contains(token));
        let starts_with_keyword = line
            .text
            .trim_start()
            .starts_with(|character: char| is_identifier_char(character));
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

/// Append to each chunk of a multi-chunk change a bounded list of where the
/// names it defines appear in the other chunks.
pub fn annotate_cross_references(chunks: &mut [DiffChunk]) {
    if chunks.len() < 2 {
        return;
    }
    let notes: Vec<String> = {
        let parsed: Vec<Vec<NewLine<'_>>> =
            chunks.iter().map(|chunk| new_lines(&chunk.text)).collect();
        parsed
            .iter()
            .enumerate()
            .map(|(index, lines)| {
                let mut entries = Vec::new();
                for name in defined_names(lines) {
                    let mentions = parsed
                        .iter()
                        .enumerate()
                        .filter(|(other, _)| *other != index)
                        .flat_map(|(_, other)| other.iter())
                        .filter(|line| identifiers(line.text).any(|token| token == name))
                        .take(MAX_PER_NAME);
                    for mention in mentions {
                        entries.push(format!(
                            "- `{name}`: {}:{} `{}`",
                            mention.path,
                            mention.line,
                            excerpt(mention.text)
                        ));
                    }
                    if entries.len() >= MAX_REFERENCES_PER_CHUNK {
                        entries.truncate(MAX_REFERENCES_PER_CHUNK);
                        break;
                    }
                }
                if entries.is_empty() {
                    return String::new();
                }
                let mut note = String::from(
                    "\n[cururu: names defined in this chunk also appear in other chunks of this change]\n",
                );
                for entry in entries {
                    let _ = writeln!(note, "{entry}");
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
        annotate_cross_references(&mut chunks);
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
        annotate_cross_references(&mut single);
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
        annotate_cross_references(&mut chunks);
        assert!(!chunks[0].text.contains("[cururu:"), "{}", chunks[0].text);
    }

    #[test]
    fn keywords_inside_expressions_or_strings_are_not_definitions() {
        let mut chunks = vec![
            chunk(
                0,
                "a.rs",
                "@@ -0,0 +1,3 @@\n+let value = json!({\"type\": \"text\"});\n+    pub(crate) async fn load_config() {}\n+export default class Widget {}\n",
            ),
            chunk(
                1,
                "b.rs",
                "@@ -0,0 +1 @@\n+use(text, load_config, Widget);\n",
            ),
        ];
        annotate_cross_references(&mut chunks);
        let note = &chunks[0].text;
        assert!(note.contains("- `load_config`: b.rs:1"), "{note}");
        assert!(note.contains("- `Widget`: b.rs:1"), "{note}");
        assert!(!note.contains("- `text`"), "{note}");
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
        annotate_cross_references(&mut chunks);
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
