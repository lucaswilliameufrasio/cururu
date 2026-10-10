pub use cururu_core::{ChangedFile, chunk_files, is_valid_anchor, parse_unified_diff};
use globset::GlobSet;

pub fn filter_ignored(files: Vec<ChangedFile>, ignore: &GlobSet) -> Vec<ChangedFile> {
    files
        .into_iter()
        .filter(|f| !ignore.is_match(&f.path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use globset::GlobSetBuilder;

    #[test]
    fn parses_multiple_files() {
        let diff = "diff --git a/a.rs b/a.rs\n+one\ndiff --git a/b.rs b/b.rs\n+two\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!(files[1].path, "b.rs");
    }

    #[test]
    fn ignores_lockfiles() {
        let mut builder = GlobSetBuilder::new();
        builder.add(globset::Glob::new("**/Cargo.lock").unwrap());
        let set = builder.build().unwrap();
        let files = vec![ChangedFile {
            path: "Cargo.lock".into(),
            patch: "x".into(),
            right_lines: vec![],
        }];
        assert_eq!(filter_ignored(files, &set).len(), 0);
    }

    #[test]
    fn tracks_added_and_context_lines() {
        // Hunk starts at new line 10. Three additions + one context line.
        let diff = "\
diff --git a/a.rs b/a.rs
@@ -1,0 +10,4 @@
+fn alpha() {}
+fn beta() {}
+fn gamma() {}
 fn kept() {}
";
        let files = parse_unified_diff(diff);
        assert_eq!(files[0].right_lines, vec![10, 11, 12, 13]);
    }

    #[test]
    fn skips_deletions_on_right_side() {
        let diff = "\
diff --git a/a.rs b/a.rs
@@ -5,3 +5,3 @@
-old_line
 fn ctx() {}
+fn added() {}
";
        let files = parse_unified_diff(diff);
        // Deletion does not advance the new side, so lines are 5 (ctx), 6 (added).
        assert_eq!(files[0].right_lines, vec![5, 6]);
    }

    #[test]
    fn validates_anchors() {
        let files = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
@@ -1,0 +1,1 @@
+fn main() {}
",
        );
        assert!(is_valid_anchor(&files, "a.rs", 1));
        assert!(!is_valid_anchor(&files, "a.rs", 2));
        assert!(!is_valid_anchor(&files, "other.rs", 1));
    }
}
