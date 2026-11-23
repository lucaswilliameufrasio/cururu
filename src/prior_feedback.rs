//! Deterministic bounds for prior review feedback sent to the model.
//!
//! Earlier discussion is useful context but must never stop a review: every
//! limit here cuts the input instead of failing. The PR conversation after the
//! latest summary has priority over inline threads, open threads over resolved
//! ones, and newer threads over older ones.
//!
//! A resolved thread with a human reply is a decision recorded by the author
//! or a reviewer. It is sent in compact form (the finding's title line, its
//! location and the latest human reply), and a new finding that repeats it on
//! unchanged code is dropped after parsing ([`suppress_decided`]).

use crate::scm::{PriorReviewComment, PriorReviewFeedback};
use cururu_core::DecidedFinding;

/// Inline review threads kept, newest first.
pub const MAX_THREADS: usize = 20;
/// Replies kept per inline thread (the latest ones), besides its root.
pub const MAX_REPLIES_PER_THREAD: usize = 10;
/// PR conversation comments kept (the latest ones).
pub const MAX_CONVERSATION_COMMENTS: usize = 10;
/// Characters kept per comment body, including the truncation mark.
pub const MAX_COMMENT_CHARS: usize = 2_000;
/// Characters kept per location label.
pub const MAX_LOCATION_CHARS: usize = 512;
/// Lines a new finding may be from a decided thread's line and still count
/// as the same place.
pub const DECIDED_LINE_WINDOW: u32 = 3;
/// Total bytes of locations and comment bodies.
pub const MAX_FEEDBACK_BYTES: usize = 12_000;
/// Appended to a body or location cut at its character limit.
pub const TRUNCATION_MARK: &str = "…[truncado]";

/// What the bounds removed, for the operator notice.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FeedbackCuts {
    /// Resolved threads left out: without a human reply, or every resolved
    /// thread when `[review].prior_feedback_resolved = "none"`.
    pub resolved_threads: usize,
    /// Resolved threads with a human reply sent as recorded decisions.
    pub resolved_kept: usize,
    /// Resolved threads with a human reply cut by [`MAX_THREADS`] or the byte
    /// budget (they are cut before any open thread).
    pub dropped_resolved_threads: usize,
    /// Open threads beyond [`MAX_THREADS`] or the byte budget.
    pub dropped_threads: usize,
    /// Thread replies and conversation comments beyond their count limit or
    /// the byte budget.
    pub dropped_comments: usize,
    /// Comment bodies cut at their character limit.
    pub truncated_comments: usize,
    /// Location labels cut at their character limit.
    pub truncated_locations: usize,
}

impl FeedbackCuts {
    pub const fn any(&self) -> bool {
        self.dropped_threads > 0
            || self.dropped_resolved_threads > 0
            || self.dropped_comments > 0
            || self.truncated_comments > 0
            || self.truncated_locations > 0
    }

    pub fn summary(&self) -> String {
        format!(
            "prior review feedback was cut to fit its limits: {} open thread(s), {} resolved thread(s) and {} comment(s) dropped, {} comment(s) and {} location(s) truncated, {} resolved thread(s) skipped",
            self.dropped_threads,
            self.dropped_resolved_threads,
            self.dropped_comments,
            self.truncated_comments,
            self.truncated_locations,
            self.resolved_threads
        )
    }
}

/// How a resolved thread with a human reply enters the feedback.
#[derive(Debug, Clone)]
pub struct Resolution {
    /// The latest reply written by a person (not a bot, not Cururu).
    pub last_human_reply: PriorReviewComment,
    /// The host marked the thread outdated: its code changed afterwards.
    pub outdated: bool,
}

/// One inline thread: its root comment id (newer threads have larger ids),
/// path, location, root comment and replies in chronological order.
/// `resolution` is set for a resolved thread kept as a decision; its replies
/// are then replaced by the latest human reply.
#[derive(Debug, Clone)]
pub struct InlineThread {
    pub root_id: u64,
    pub path: String,
    /// The root comment's line on the current diff, if any.
    pub line: Option<u32>,
    pub location: String,
    pub root: PriorReviewComment,
    pub replies: Vec<PriorReviewComment>,
    pub resolution: Option<Resolution>,
}

impl InlineThread {
    pub const fn resolved(&self) -> bool {
        self.resolution.is_some()
    }
}

/// Cut `value` to at most `max_chars` characters on a character boundary,
/// ending with [`TRUNCATION_MARK`] when it was cut.
pub fn truncate_chars(value: &str, max_chars: usize) -> (String, bool) {
    if value.chars().count() <= max_chars {
        return (value.to_owned(), false);
    }
    let mark_chars = TRUNCATION_MARK.chars().count();
    if max_chars < mark_chars {
        // No room for the mark: keep the limit, without it.
        return (value.chars().take(max_chars).collect(), true);
    }
    let mut cut: String = value.chars().take(max_chars - mark_chars).collect();
    cut.push_str(TRUNCATION_MARK);
    (cut, true)
}

fn bounded_comment(comment: PriorReviewComment, cuts: &mut FeedbackCuts) -> PriorReviewComment {
    let (body, truncated) = truncate_chars(&comment.body, MAX_COMMENT_CHARS);
    if truncated {
        cuts.truncated_comments += 1;
    }
    PriorReviewComment { body, ..comment }
}

fn bounded_location(location: &str, cuts: &mut FeedbackCuts) -> String {
    let (location, truncated) = truncate_chars(location, MAX_LOCATION_CHARS);
    if truncated {
        cuts.truncated_locations += 1;
    }
    location
}

/// The first non-empty line of a Cururu finding comment, without its marker:
/// `**SEVERITY**: title`.
pub fn finding_title_line(body: &str) -> &str {
    body.lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(crate::output::finding_marker())
                .trim()
        })
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// The title of a finding title line, without its `**SEVERITY**:` prefix.
pub fn finding_title(title_line: &str) -> &str {
    title_line
        .strip_prefix("**")
        .and_then(|rest| rest.split_once("**:"))
        .map_or(title_line, |(_, title)| title)
        .trim()
}

/// Titles compared case- and whitespace-insensitively, ignoring a trailing
/// period.
pub fn normalized_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches('.')
        .to_lowercase()
}

/// Bytes counted against [`MAX_FEEDBACK_BYTES`]: the location, each
/// comment's author and body, and a decided finding's path and title, as
/// sent to the model.
pub fn feedback_bytes(feedback: &PriorReviewFeedback) -> usize {
    feedback.location.len()
        + feedback
            .comments
            .iter()
            .map(|comment| comment.author.len() + comment.body.len())
            .sum::<usize>()
        + feedback
            .decided
            .as_ref()
            .map_or(0, |decided| decided.path.len() + decided.title.len())
}

fn open_entry(thread: InlineThread, cuts: &mut FeedbackCuts) -> PriorReviewFeedback {
    let mut replies = thread.replies;
    if replies.len() > MAX_REPLIES_PER_THREAD {
        let excess = replies.len() - MAX_REPLIES_PER_THREAD;
        cuts.dropped_comments += excess;
        replies.drain(..excess);
    }
    let mut comments = Vec::with_capacity(replies.len() + 1);
    comments.push(bounded_comment(thread.root, cuts));
    comments.extend(
        replies
            .into_iter()
            .map(|reply| bounded_comment(reply, cuts)),
    );
    PriorReviewFeedback {
        location: bounded_location(&thread.location, cuts),
        comments,
        resolved: false,
        decided: None,
    }
}

/// Compact form of a resolved thread: the finding's title line, its location
/// and the latest human reply.
fn resolved_entry(
    thread: InlineThread,
    resolution: Resolution,
    cuts: &mut FeedbackCuts,
) -> PriorReviewFeedback {
    let title_line = finding_title_line(&thread.root.body).to_owned();
    let (title, _) = truncate_chars(finding_title(&title_line), MAX_LOCATION_CHARS);
    let (path, _) = truncate_chars(&thread.path, MAX_LOCATION_CHARS);
    PriorReviewFeedback {
        location: bounded_location(&thread.location, cuts),
        comments: vec![
            bounded_comment(
                PriorReviewComment {
                    author: thread.root.author,
                    body: title_line,
                },
                cuts,
            ),
            bounded_comment(resolution.last_human_reply, cuts),
        ],
        resolved: true,
        decided: Some(DecidedFinding {
            path,
            line: thread.line,
            title,
            code_changed: resolution.outdated,
        }),
    }
}

/// Apply every limit: keep the newest [`MAX_THREADS`] threads, open ones
/// first, and the latest replies and conversation comments, truncate long
/// bodies, then fill the byte budget with the conversation first, open
/// threads from newest to oldest and resolved threads from newest to oldest,
/// dropping whole threads that do not fit. Threads are returned oldest first,
/// followed by the conversation.
pub fn bound_feedback(
    threads: Vec<InlineThread>,
    conversation: Option<PriorReviewFeedback>,
    mut cuts: FeedbackCuts,
) -> (Vec<PriorReviewFeedback>, FeedbackCuts) {
    let (mut open, mut resolved): (Vec<_>, Vec<_>) =
        threads.into_iter().partition(|thread| !thread.resolved());
    open.sort_by_key(|thread| std::cmp::Reverse(thread.root_id));
    resolved.sort_by_key(|thread| std::cmp::Reverse(thread.root_id));
    if open.len() > MAX_THREADS {
        cuts.dropped_threads += open.len() - MAX_THREADS;
        open.truncate(MAX_THREADS);
    }
    let resolved_slots = MAX_THREADS - open.len();
    if resolved.len() > resolved_slots {
        cuts.dropped_resolved_threads += resolved.len() - resolved_slots;
        resolved.truncate(resolved_slots);
    }
    // Open threads first, then resolved ones: the budget is filled in this
    // order, so a resolved thread never displaces an open one.
    let mut candidates: Vec<(u64, PriorReviewFeedback)> = Vec::new();
    for thread in open.into_iter().chain(resolved) {
        let root_id = thread.root_id;
        let entry = match thread.resolution.clone() {
            Some(resolution) => resolved_entry(thread, resolution, &mut cuts),
            None => open_entry(thread, &mut cuts),
        };
        candidates.push((root_id, entry));
    }

    let mut budget = MAX_FEEDBACK_BYTES;
    let conversation = conversation.and_then(|conversation| {
        let mut comments = conversation.comments;
        if comments.len() > MAX_CONVERSATION_COMMENTS {
            let excess = comments.len() - MAX_CONVERSATION_COMMENTS;
            cuts.dropped_comments += excess;
            comments.drain(..excess);
        }
        let mut conversation = PriorReviewFeedback {
            location: bounded_location(&conversation.location, &mut cuts),
            comments: comments
                .into_iter()
                .map(|comment| bounded_comment(comment, &mut cuts))
                .collect(),
            resolved: false,
            decided: None,
        };
        // The conversation has priority, but must fit too: drop its oldest
        // comments until it does.
        while !conversation.comments.is_empty() && feedback_bytes(&conversation) > budget {
            conversation.comments.remove(0);
            cuts.dropped_comments += 1;
        }
        if conversation.comments.is_empty() {
            return None;
        }
        budget -= feedback_bytes(&conversation);
        Some(conversation)
    });

    let mut kept = Vec::new();
    for (root_id, entry) in candidates {
        let bytes = feedback_bytes(&entry);
        if bytes <= budget {
            budget -= bytes;
            if entry.resolved {
                cuts.resolved_kept += 1;
            }
            kept.push((root_id, entry));
        } else if entry.resolved {
            cuts.dropped_resolved_threads += 1;
        } else {
            cuts.dropped_threads += 1;
        }
    }
    kept.sort_by_key(|(root_id, _)| *root_id);
    let mut kept: Vec<_> = kept.into_iter().map(|(_, entry)| entry).collect();
    kept.extend(conversation);
    (kept, cuts)
}

/// Final check that the bounded feedback respects every limit. It must never
/// fail after [`bound_feedback`]; a failure is a bug in the bounds.
pub fn validate_bounds(feedback: &[PriorReviewFeedback]) -> anyhow::Result<()> {
    let mut total = 0usize;
    for entry in feedback {
        anyhow::ensure!(
            entry.location.chars().count() <= MAX_LOCATION_CHARS,
            "prior feedback location exceeds character limit"
        );
        anyhow::ensure!(
            entry
                .comments
                .iter()
                .all(|comment| comment.body.chars().count() <= MAX_COMMENT_CHARS),
            "prior feedback comment exceeds character limit"
        );
        anyhow::ensure!(
            entry.decided.as_ref().is_none_or(|decided| {
                decided.path.chars().count() <= MAX_LOCATION_CHARS
                    && decided.title.chars().count() <= MAX_LOCATION_CHARS
            }),
            "prior feedback decided finding exceeds character limit"
        );
        total = total.saturating_add(feedback_bytes(entry));
    }
    anyhow::ensure!(
        total <= MAX_FEEDBACK_BYTES,
        "prior feedback exceeds byte limit"
    );
    Ok(())
}

/// Drop new findings that repeat a decided one: same file, a line within
/// [`DECIDED_LINE_WINDOW`] of the thread's and the same normalized title as a
/// resolved thread with a human reply whose code did not change since. A
/// finding or thread without a line is never dropped, so a distinct
/// occurrence elsewhere in the file reaches the review. Returns how many
/// were dropped; each is logged as `suppressed_as_decided`.
pub fn suppress_decided(
    feedback: &[PriorReviewFeedback],
    review: &mut cururu_core::ReviewResult,
) -> usize {
    let decided: Vec<(&str, u32, String)> = feedback
        .iter()
        .filter(|entry| entry.resolved)
        .filter_map(|entry| entry.decided.as_ref())
        .filter(|decided| !decided.code_changed && !decided.title.trim().is_empty())
        .filter_map(|decided| {
            Some((
                decided.path.as_str(),
                decided.line?,
                normalized_title(&decided.title),
            ))
        })
        .collect();
    if decided.is_empty() {
        return 0;
    }
    let before = review.findings.len();
    review.findings.retain(|finding| {
        let title = normalized_title(&finding.title);
        let repeated = !title.is_empty()
            && finding.line.is_some_and(|line| {
                decided.iter().any(|(path, decided_line, decided)| {
                    *path == finding.path
                        && line.abs_diff(*decided_line) <= DECIDED_LINE_WINDOW
                        && *decided == title
                })
            });
        if repeated {
            tracing::info!(
                path = %finding.path,
                title = %finding.title,
                "suppressed_as_decided: finding repeats a resolved thread with a human reply on unchanged code"
            );
        }
        !repeated
    });
    before - review.findings.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(body: &str) -> PriorReviewComment {
        PriorReviewComment {
            author: "fixture".into(),
            body: body.into(),
        }
    }

    fn thread(root_id: u64, replies: usize, body: &str) -> InlineThread {
        InlineThread {
            root_id,
            path: format!("src/{root_id}.rs"),
            line: Some(5),
            location: format!("src/{root_id}.rs (line 1)"),
            root: comment("finding"),
            replies: (0..replies)
                .map(|index| comment(&format!("{index}:{body}")))
                .collect(),
            resolution: None,
        }
    }

    /// A resolved thread whose latest human reply is `reply`.
    fn decided(root_id: u64, reply: &str) -> InlineThread {
        InlineThread {
            root: comment(&format!(
                "<!-- cururu:finding -->\n\n**MEDIUM**: Finding {root_id}\n\nLong explanation that is not sent."
            )),
            resolution: Some(Resolution {
                last_human_reply: comment(reply),
                outdated: false,
            }),
            ..thread(root_id, 0, "")
        }
    }

    fn finding(path: &str, title: &str) -> cururu_core::ReviewFinding {
        cururu_core::ReviewFinding {
            severity: "medium".into(),
            path: path.into(),
            line: Some(5),
            title: title.into(),
            message: "message".into(),
            suggestion: "suggestion".into(),
            confidence: 0.9,
            suggested_change: None,
            source: None,
            rule: None,
        }
    }

    #[test]
    fn truncation_respects_utf8_boundaries_and_marks_the_cut() {
        let body = "é".repeat(MAX_COMMENT_CHARS + 5);
        let (cut, truncated) = truncate_chars(&body, MAX_COMMENT_CHARS);
        assert!(truncated);
        assert_eq!(cut.chars().count(), MAX_COMMENT_CHARS);
        assert!(cut.ends_with(TRUNCATION_MARK));
        assert!(
            cut.trim_end_matches(TRUNCATION_MARK)
                .chars()
                .all(|c| c == 'é')
        );
        assert_eq!(truncate_chars("ação", 4), ("ação".to_owned(), false));
        // A limit smaller than the mark still holds, without the mark.
        assert_eq!(truncate_chars("ação longa", 3), ("açã".to_owned(), true));
    }

    #[test]
    fn budget_keeps_the_conversation_and_the_newest_threads() {
        let threads: Vec<_> = (1..=10)
            .map(|id| thread(id, 1, &"x".repeat(1_900)))
            .collect();
        let conversation = PriorReviewFeedback {
            location: "PR conversation".into(),
            comments: vec![comment("please also check the retry")],
            resolved: false,
            decided: None,
        };
        let (feedback, cuts) = bound_feedback(threads, Some(conversation), FeedbackCuts::default());
        validate_bounds(&feedback).unwrap();
        assert_eq!(feedback.last().unwrap().location, "PR conversation");
        let kept: Vec<_> = feedback[..feedback.len() - 1]
            .iter()
            .map(|entry| entry.location.clone())
            .collect();
        // Six threads of ~1.9 KB fit beside the conversation; the newest ones,
        // returned oldest first.
        assert_eq!(kept.len(), 6);
        assert_eq!(kept.first().unwrap(), "src/5.rs (line 1)");
        assert_eq!(kept.last().unwrap(), "src/10.rs (line 1)");
        assert_eq!(cuts.dropped_threads, 4);
        assert!(cuts.any());
    }

    #[test]
    fn counts_and_lengths_are_cut_not_rejected() {
        let mut threads: Vec<_> = (1..=25).map(|id| thread(id, 1, "ok")).collect();
        threads.push(thread(100, 15, "reply"));
        threads.push(InlineThread {
            root_id: 101,
            path: "x".into(),
            line: None,
            location: "x".repeat(600),
            root: comment(&"ç".repeat(2_500)),
            replies: vec![comment("short")],
            resolution: None,
        });
        let (feedback, cuts) = bound_feedback(threads, None, FeedbackCuts::default());
        validate_bounds(&feedback).unwrap();
        assert!(feedback.len() <= MAX_THREADS);
        let many = feedback
            .iter()
            .find(|entry| entry.location == "src/100.rs (line 1)")
            .unwrap();
        assert_eq!(many.comments.len(), MAX_REPLIES_PER_THREAD + 1);
        assert_eq!(many.comments[1].body, "5:reply");
        let long = feedback
            .iter()
            .find(|entry| entry.location.ends_with(TRUNCATION_MARK))
            .unwrap();
        assert_eq!(long.location.chars().count(), MAX_LOCATION_CHARS);
        assert!(long.comments[0].body.ends_with(TRUNCATION_MARK));
        assert_eq!(cuts.dropped_threads, 7);
        assert_eq!(cuts.dropped_comments, 5);
        assert_eq!(cuts.truncated_comments, 1);
        assert_eq!(cuts.truncated_locations, 1);
        assert!(
            cuts.summary()
                .contains("1 comment(s) and 1 location(s) truncated")
        );
    }

    #[test]
    fn an_oversized_conversation_keeps_its_latest_comments() {
        let conversation = PriorReviewFeedback {
            location: "PR conversation".into(),
            comments: (0..12)
                .map(|index| comment(&format!("{index}{}", "é".repeat(1_990))))
                .collect(),
            resolved: false,
            decided: None,
        };
        let (feedback, cuts) =
            bound_feedback(Vec::new(), Some(conversation), FeedbackCuts::default());
        validate_bounds(&feedback).unwrap();
        let kept = &feedback[0].comments;
        assert!(kept.last().unwrap().body.starts_with("11"));
        assert_eq!(cuts.dropped_comments, 12 - kept.len());
    }

    #[test]
    fn authors_count_against_the_budget() {
        let long_author = "a".repeat(39);
        let threads: Vec<_> = (1..=20)
            .map(|id| InlineThread {
                root_id: id,
                path: format!("src/{id}.rs"),
                line: Some(1),
                resolution: None,
                location: format!("src/{id}.rs (line 1)"),
                root: PriorReviewComment {
                    author: long_author.clone(),
                    body: "f".repeat(300),
                },
                replies: (0..10)
                    .map(|_| PriorReviewComment {
                        author: long_author.clone(),
                        body: "r".repeat(20),
                    })
                    .collect(),
            })
            .collect();
        let (feedback, cuts) = bound_feedback(threads, None, FeedbackCuts::default());
        validate_bounds(&feedback).unwrap();
        let rendered: usize = feedback
            .iter()
            .map(|entry| {
                entry.location.len()
                    + entry
                        .comments
                        .iter()
                        .map(|c| c.author.len() + c.body.len())
                        .sum::<usize>()
            })
            .sum();
        assert!(rendered <= MAX_FEEDBACK_BYTES, "{rendered}");
        // Bodies and locations alone (~10.4 KB) fit all 20 threads; the
        // 39-byte authors (~8.6 KB more) do not, so threads must be dropped.
        assert!(cuts.dropped_threads > 0);
    }

    #[test]
    fn validation_still_reports_out_of_bounds_input() {
        let mut feedback = vec![PriorReviewFeedback {
            location: "a".into(),
            comments: vec![comment(&"é".repeat(MAX_FEEDBACK_BYTES))],
            resolved: false,
            decided: None,
        }];
        assert!(validate_bounds(&feedback).is_err());
        feedback[0].comments[0].body = "ok".into();
        assert!(validate_bounds(&feedback).is_ok());
        feedback[0].location = "x".repeat(MAX_LOCATION_CHARS + 1);
        assert!(validate_bounds(&feedback).is_err());
    }

    #[test]
    fn resolved_threads_are_sent_compact_as_decisions() {
        let long_reply = "ã".repeat(MAX_COMMENT_CHARS + 50);
        let (feedback, cuts) = bound_feedback(
            vec![decided(7, &long_reply), thread(3, 1, "open")],
            None,
            FeedbackCuts::default(),
        );
        validate_bounds(&feedback).unwrap();
        // Oldest first: the open thread (3), then the decision (7).
        assert!(!feedback[0].resolved && feedback[0].decided.is_none());
        let decision = &feedback[1];
        assert!(decision.resolved);
        assert_eq!(decision.location, "src/7.rs (line 1)");
        // Only the title line and the latest human reply, truncated.
        assert_eq!(decision.comments.len(), 2);
        assert_eq!(decision.comments[0].body, "**MEDIUM**: Finding 7");
        assert_eq!(decision.comments[1].body.chars().count(), MAX_COMMENT_CHARS);
        assert!(decision.comments[1].body.ends_with(TRUNCATION_MARK));
        assert_eq!(
            decision.decided,
            Some(DecidedFinding {
                path: "src/7.rs".into(),
                line: Some(5),
                title: "Finding 7".into(),
                code_changed: false,
            })
        );
        assert_eq!(cuts.resolved_kept, 1);
        assert_eq!(cuts.truncated_comments, 1);
    }

    #[test]
    fn the_budget_cuts_resolved_threads_before_open_ones_oldest_first() {
        // Five open threads of ~1.9 KB use most of the 12 KB budget; the
        // decisions are newer, but only the newest ones still fit.
        let mut threads: Vec<_> = (1..=5)
            .map(|id| thread(id, 1, &"o".repeat(1_900)))
            .collect();
        threads.extend((10..=15).map(|id| decided(id, &"r".repeat(400))));
        let (feedback, cuts) = bound_feedback(threads, None, FeedbackCuts::default());
        validate_bounds(&feedback).unwrap();
        let open = feedback.iter().filter(|entry| !entry.resolved).count();
        assert_eq!(open, 5, "no open thread is dropped for a decision");
        let kept: Vec<_> = feedback
            .iter()
            .filter(|entry| entry.resolved)
            .map(|entry| entry.location.clone())
            .collect();
        assert!(!kept.is_empty() && kept.len() < 6, "{kept:?}");
        assert_eq!(kept.last().unwrap(), "src/15.rs (line 1)");
        let first_kept = 16 - kept.len();
        assert_eq!(kept[0], format!("src/{first_kept}.rs (line 1)"));
        assert_eq!(cuts.dropped_threads, 0);
        assert_eq!(cuts.dropped_resolved_threads, 6 - kept.len());
        assert_eq!(cuts.resolved_kept, kept.len());
        assert!(cuts.any());
    }

    #[test]
    fn the_thread_count_limit_cuts_resolved_threads_first() {
        let mut threads: Vec<_> = (1..=18).map(|id| thread(id, 1, "ok")).collect();
        threads.extend((100..=104).map(|id| decided(id, "kept as is")));
        let (feedback, cuts) = bound_feedback(threads, None, FeedbackCuts::default());
        validate_bounds(&feedback).unwrap();
        assert_eq!(feedback.len(), MAX_THREADS);
        let kept: Vec<_> = feedback
            .iter()
            .filter(|entry| entry.resolved)
            .map(|entry| entry.location.as_str())
            .collect();
        assert_eq!(kept, ["src/103.rs (line 1)", "src/104.rs (line 1)"]);
        assert_eq!(cuts.dropped_resolved_threads, 3);
        assert_eq!(cuts.dropped_threads, 0);
    }

    #[test]
    fn finding_titles_are_read_from_the_comment_body() {
        assert_eq!(
            finding_title_line("<!-- cururu:finding -->\n\n**HIGH**: Path traversal\n\nmore"),
            "**HIGH**: Path traversal"
        );
        assert_eq!(finding_title_line("<!-- cururu:finding --> plain"), "plain");
        assert_eq!(finding_title("**HIGH**: Path traversal"), "Path traversal");
        assert_eq!(finding_title("No severity"), "No severity");
        assert_eq!(
            normalized_title("  Catching   every Exception. "),
            normalized_title("catching every exception")
        );
    }

    #[test]
    fn repeated_findings_on_unchanged_decided_code_are_suppressed() {
        let (feedback, _) = bound_feedback(
            vec![decided(1, "Not applicable: intentional.")],
            None,
            FeedbackCuts::default(),
        );
        let mut review = cururu_core::ReviewResult {
            model: "fixture".into(),
            files_reviewed: 1,
            summary: String::new(),
            findings: vec![
                finding("src/1.rs", "  finding   1."),
                finding("src/1.rs", "Another problem"),
                finding("src/2.rs", "Finding 1"),
            ],
        };
        assert_eq!(suppress_decided(&feedback, &mut review), 1);
        let titles: Vec<_> = review.findings.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(titles, ["Another problem", "Finding 1"]);

        // Code changed after the decision: the finding stays.
        let mut changed = feedback.clone();
        changed[0].decided.as_mut().unwrap().code_changed = true;
        let mut review = cururu_core::ReviewResult {
            findings: vec![finding("src/1.rs", "Finding 1")],
            ..review
        };
        assert_eq!(suppress_decided(&changed, &mut review), 0);
        assert_eq!(review.findings.len(), 1);
    }

    #[test]
    fn a_distinct_occurrence_elsewhere_in_the_file_is_kept() {
        // The decision is on line 5; same file and title elsewhere is a new
        // occurrence the review must see.
        let (feedback, _) = bound_feedback(
            vec![decided(1, "Not applicable: intentional.")],
            None,
            FeedbackCuts::default(),
        );
        let at = |line: Option<u32>| cururu_core::ReviewFinding {
            line,
            ..finding("src/1.rs", "Finding 1")
        };
        let mut review = cururu_core::ReviewResult {
            model: "fixture".into(),
            files_reviewed: 1,
            summary: String::new(),
            findings: vec![at(Some(8)), at(Some(9)), at(Some(40)), at(None)],
        };
        assert_eq!(suppress_decided(&feedback, &mut review), 1);
        let lines: Vec<_> = review.findings.iter().map(|f| f.line).collect();
        assert_eq!(lines, [Some(9), Some(40), None]);

        // A decision without a line suppresses nothing.
        let mut unanchored = feedback.clone();
        unanchored[0].decided.as_mut().unwrap().line = None;
        let mut review = cururu_core::ReviewResult {
            findings: vec![at(Some(5))],
            ..review
        };
        assert_eq!(suppress_decided(&unanchored, &mut review), 0);
    }
}
