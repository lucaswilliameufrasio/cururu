use crate::ChangedFile;

/// Parse Git's unified diff into changed files and their new-side line anchors.
#[must_use]
pub fn parse_unified_diff(diff: &str) -> Vec<ChangedFile> {
    let mut files = Vec::new();
    let mut current_file: Option<(String, String)> = None;

    for raw_line in diff.split_inclusive('\n') {
        let line = without_line_ending(raw_line);
        if let Some(path) = file_path_from_header(line) {
            if let Some((path, patch)) = current_file.take() {
                files.push(changed_file(path, patch));
            }
            current_file = Some((path.to_string(), raw_line.to_string()));
        } else if let Some((_, patch)) = &mut current_file {
            patch.push_str(raw_line);
        }
    }

    if let Some((path, patch)) = current_file {
        files.push(changed_file(path, patch));
    }

    files
}

fn file_path_from_header(line: &str) -> Option<&str> {
    let remainder = line.strip_prefix("diff --git a/")?;
    let separator = remainder.find(" b/")?;
    Some(&remainder[separator + " b/".len()..])
}

fn changed_file(file_path: String, diff_patch: String) -> ChangedFile {
    let right_lines = new_file_right_lines(&diff_patch);
    ChangedFile {
        path: file_path,
        patch: diff_patch,
        right_lines,
    }
}

fn new_file_right_lines(diff: &str) -> Vec<u32> {
    let mut lines = Vec::new();
    let mut current_line = None;

    for raw_line in diff.split_inclusive('\n') {
        let line = without_line_ending(raw_line);
        if let Some(new_start) = new_hunk_start(line) {
            current_line = Some(new_start);
            continue;
        }

        let Some(first_character) = line.chars().next() else {
            continue;
        };
        let Some(new_line) = current_line.as_mut() else {
            continue;
        };

        match first_character {
            '+' | ' ' => {
                lines.push(*new_line);
                *new_line += 1;
            }
            '-' | '\\' => {}
            _ => current_line = None,
        }
    }

    lines
}

fn new_hunk_start(line: &str) -> Option<u32> {
    let remainder = line.strip_prefix("@@ -")?;
    let (old_range, new_range) = remainder.split_once(" +")?;
    if !valid_range(old_range) {
        return None;
    }

    let digit_count = new_range.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }

    let start = new_range[..digit_count].parse().unwrap_or(0);
    let mut suffix = &new_range[digit_count..];
    if let Some(count) = suffix.strip_prefix(',') {
        let count_length = count.bytes().take_while(u8::is_ascii_digit).count();
        if count_length == 0 {
            return None;
        }
        suffix = &count[count_length..];
    }

    suffix.strip_prefix(" @@")?;
    Some(start)
}

fn valid_range(range: &str) -> bool {
    let mut parts = range.split(',');
    let Some(start) = parts.next() else {
        return false;
    };
    if !start.bytes().all(|byte| byte.is_ascii_digit()) || start.is_empty() {
        return false;
    }
    if let Some(count) = parts.next() {
        if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        if parts.next().is_some() {
            return false;
        }
    }
    true
}

fn without_line_ending(line: &str) -> &str {
    let without_newline = line.strip_suffix('\n').unwrap_or(line);
    without_newline
        .strip_suffix('\r')
        .unwrap_or(without_newline)
}

#[cfg(test)]
mod tests {
    use super::parse_unified_diff;

    #[test]
    fn parses_multiple_files_and_paths_with_spaces() {
        let diff = "diff --git a/a.rs b/a.rs\n+one\ndiff --git a/a file.rs b/a file.rs\n+two\n";

        let files = parse_unified_diff(diff);

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!(files[1].path, "a file.rs");
        assert_eq!(files[1].patch, "diff --git a/a file.rs b/a file.rs\n+two\n");
    }

    #[test]
    fn tracks_context_addition_and_deletion_lines_across_hunks() {
        let diff = concat!(
            "diff --git a/a.rs b/a.rs\n",
            "@@ -1,2 +10,3 @@ first\n",
            " context\n",
            "-deleted\n",
            "+added\n",
            "+another\n",
            "@@ -20 +40 @@ second\n",
            "+next\n",
        );

        let files = parse_unified_diff(diff);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].right_lines, [10, 11, 12, 40]);
    }

    #[test]
    fn accepts_crlf_headers_and_ignores_malformed_hunks() {
        let diff =
            "diff --git a/a.rs b/a.rs\r\n@@ -x +4 @@\r\n+ignored\r\n@@ -1 +7 @@ valid\r\n+kept\r\n";

        let files = parse_unified_diff(diff);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].right_lines, [7]);
    }
}
