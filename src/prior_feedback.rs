//! Deterministic bounds for prior review feedback sent to the model.
//!
//! Earlier discussion is useful context but must never stop a review: every
//! limit here cuts the input instead of failing. The PR conversation after the
//! latest summary has priority over inline threads, open threads over resolved
//! ones, and newer threads over older ones.
//!
//! A resolved thread with a human reply is a decision recorded by the author
//! or a reviewer. It is sent in compact form (the finding's title line, its
//! location and the latest human replies), and a new finding that repeats it on
//! unchanged code is dropped after parsing ([`suppress_decided`]).
//!
//! Resolved threads on files in the current diff, then newer ones, come first.
//! Those beyond `[review].prior_feedback_max_resolved` or the byte budget are
//! not dropped: they are sent as one-line summaries (title line and the start
//! of the most explanatory reply) within `[review].prior_feedback_summary_bytes`,
//! so the model still sees every decision it could otherwise reopen.

use crate::scm::{PriorReviewComment, PriorReviewFeedback};
use cururu_core::{CodeAnchor, DecidedFinding};
use sha2::{Digest, Sha256};

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
/// Default for `[review].prior_feedback_max_resolved`.
pub const DEFAULT_MAX_RESOLVED: usize = 20;
/// Highest accepted `[review].prior_feedback_max_resolved`.
pub const LIMIT_MAX_RESOLVED: usize = MAX_THREADS;
/// Default for `[review].prior_feedback_summary_bytes`.
pub const DEFAULT_SUMMARY_BYTES: usize = 16_000;
/// Highest accepted `[review].prior_feedback_summary_bytes`.
pub const LIMIT_SUMMARY_BYTES: usize = 64_000;
/// Characters kept of a summarized thread's title line.
pub const SUMMARY_TITLE_CHARS: usize = 160;
/// Characters kept of a summarized thread's reply.
pub const SUMMARY_REPLY_CHARS: usize = 240;
/// Characters kept of a summarized thread's location.
pub const SUMMARY_LOCATION_CHARS: usize = 160;

/// How prior feedback is selected for one review.
#[derive(Debug, Clone)]
pub struct FeedbackOptions {
    pub resolved: crate::config::PriorFeedbackResolved,
    /// Paths in the current diff: their resolved threads come first.
    pub changed_paths: Vec<String>,
    pub max_resolved: usize,
    pub summary_bytes: usize,
}

impl FeedbackOptions {
    /// Default limits for `resolved`, with no diff paths.
    #[cfg(test)]
    pub const fn new(resolved: crate::config::PriorFeedbackResolved) -> Self {
        Self {
            resolved,
            changed_paths: Vec::new(),
            max_resolved: DEFAULT_MAX_RESOLVED,
            summary_bytes: DEFAULT_SUMMARY_BYTES,
        }
    }

    /// The configured selection for a review of `files`.
    pub fn for_review(
        review: &crate::config::ReviewConfig,
        files: &[cururu_core::ChangedFile],
    ) -> Self {
        Self {
            resolved: review.prior_feedback_resolved,
            changed_paths: files.iter().map(|file| file.path.clone()).collect(),
            max_resolved: review.prior_feedback_max_resolved,
            summary_bytes: review.prior_feedback_summary_bytes,
        }
    }
}

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
    /// Resolved threads with a human reply beyond the full budget, sent as
    /// one-line summaries.
    pub resolved_summarized: usize,
    /// Root comment ids of the summarized threads.
    pub summarized_ids: Vec<u64>,
    /// Resolved threads with a human reply that fit neither the full budget
    /// nor the summary budget (they are cut before any open thread).
    pub dropped_resolved_threads: usize,
    /// Root comment ids of the dropped resolved threads.
    pub dropped_resolved_ids: Vec<u64>,
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
        // Summarizing is not a cut: every summarized decision still reaches
        // the model. It is logged with the resolved-thread counts.
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
    /// Replies written by people (not bots, not Cururu), oldest first. The
    /// explanation is often an earlier reply than a closing "resolving".
    pub human_replies: Vec<PriorReviewComment>,
    /// The host marked the thread outdated: its code changed afterwards.
    pub outdated: bool,
}

/// One inline thread: its root comment id (newer threads have larger ids),
/// path, location, root comment and replies in chronological order.
/// `resolution` is set for a resolved thread kept as a decision; its replies
/// are then replaced by its human replies.
#[derive(Debug, Clone)]
pub struct InlineThread {
    pub root_id: u64,
    pub path: String,
    /// The root comment's line on the current diff, if any.
    pub line: Option<u32>,
    /// The commented code when the thread was written ([`anchor_from_hunk`]).
    pub anchor: Option<CodeAnchor>,
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

/// Human replies kept per resolved thread (the latest ones).
pub const MAX_DECISION_REPLIES: usize = 3;

/// Lines of code kept in a [`CodeAnchor`]: the commented line and up to this
/// many before it, minus one.
pub const ANCHOR_LINES: usize = 4;

fn anchor_of(lines: &[&str]) -> CodeAnchor {
    CodeAnchor {
        lines: u32::try_from(lines.len()).unwrap_or(u32::MAX),
        sha256: hex::encode(Sha256::digest(lines.join("\n").as_bytes())),
    }
}

/// The anchor of a review comment from its diff hunk, which ends at the
/// commented line: that line and up to [`ANCHOR_LINES`] - 1 new-side lines
/// before it. None for a comment on a removed (left-side) line or an empty
/// hunk.
///
/// With `original_line` (the commented line when the comment was written, on
/// the hunk's new side) the hunk is walked to that line, so context lines
/// after it are ignored; without it the hunk must end at the commented line,
/// as GitHub's `diff_hunk` does.
pub fn anchor_from_hunk(hunk: &str, original_line: Option<u32>) -> Option<CodeAnchor> {
    let mut body: Vec<&str> = Vec::new();
    let mut next: Option<u32> = None;
    for line in hunk.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            body.clear();
            next = header
                .split_whitespace()
                .find_map(|part| part.strip_prefix('+'))
                .and_then(|range| range.split(',').next())
                .and_then(|start| start.parse().ok());
            continue;
        }
        if next.is_none() || line.starts_with('\\') {
            continue;
        }
        body.push(line);
        if !line.starts_with('-') {
            if original_line.is_some() && next == original_line {
                break;
            }
            next = next.map(|number| number + 1);
        }
    }
    if original_line.is_some_and(|line| next != Some(line)) {
        return None;
    }
    if body.last().is_none_or(|line| line.starts_with('-')) {
        return None;
    }
    let mut lines: Vec<&str> = body
        .iter()
        .rev()
        .filter(|line| !line.starts_with('-'))
        .take(ANCHOR_LINES)
        .map(|line| line.get(1..).unwrap_or_default())
        .collect();
    lines.reverse();
    Some(anchor_of(&lines))
}

/// New-side lines of a unified patch by line number: context and added
/// lines only.
fn new_side_lines(patch: &str) -> std::collections::HashMap<u32, &str> {
    let mut lines = std::collections::HashMap::new();
    let mut next: Option<u32> = None;
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            next = header
                .split_whitespace()
                .find_map(|part| part.strip_prefix('+'))
                .and_then(|range| range.split(',').next())
                .and_then(|start| start.parse().ok());
            continue;
        }
        let Some(number) = next else {
            continue;
        };
        match line.as_bytes().first() {
            Some(b'+' | b' ') => {
                lines.insert(number, line.get(1..).unwrap_or_default());
                next = Some(number + 1);
            }
            Some(b'-' | b'\\') => {}
            _ => next = None,
        }
    }
    lines
}

/// Whether the current diff still has the anchored code ending at `line`.
fn anchor_unchanged(anchor: &CodeAnchor, line: u32, patch: &str) -> bool {
    let current = new_side_lines(patch);
    let Some(first) = (line + 1).checked_sub(anchor.lines) else {
        return false;
    };
    let lines: Option<Vec<&str>> = (first..=line)
        .map(|number| current.get(&number).copied())
        .collect();
    lines.is_some_and(|lines| anchor.lines > 0 && anchor_of(&lines) == *anchor)
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
        + feedback.decided.as_ref().map_or(0, |decided| {
            decided.path.len()
                + decided.title.len()
                + decided
                    .anchor
                    .as_ref()
                    .map_or(0, |anchor| anchor.sha256.len())
        })
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
        summarized: false,
        decided: None,
    }
}

/// Compact form of a resolved thread: the finding's title line, its location
/// and the latest [`MAX_DECISION_REPLIES`] human replies.
fn resolved_entry(
    thread: InlineThread,
    resolution: Resolution,
    cuts: &mut FeedbackCuts,
) -> PriorReviewFeedback {
    let title_line = finding_title_line(&thread.root.body).to_owned();
    let (title, _) = truncate_chars(finding_title(&title_line), MAX_LOCATION_CHARS);
    let (path, _) = truncate_chars(&thread.path, MAX_LOCATION_CHARS);
    let mut replies = resolution.human_replies;
    if replies.len() > MAX_DECISION_REPLIES {
        let excess = replies.len() - MAX_DECISION_REPLIES;
        cuts.dropped_comments += excess;
        replies.drain(..excess);
    }
    let mut comments = vec![bounded_comment(
        PriorReviewComment {
            author: thread.root.author,
            body: title_line,
        },
        cuts,
    )];
    comments.extend(
        replies
            .into_iter()
            .map(|reply| bounded_comment(reply, cuts)),
    );
    PriorReviewFeedback {
        location: bounded_location(&thread.location, cuts),
        comments,
        resolved: true,
        summarized: false,
        decided: Some(DecidedFinding {
            path,
            line: thread.line,
            anchor: thread.anchor,
            title,
            code_changed: resolution.outdated,
        }),
    }
}

/// The reply that best explains a decision: the longest of the latest
/// [`MAX_DECISION_REPLIES`] human replies (a closing "resolving" often follows
/// the explanation), the latest on a tie.
fn explanatory_reply(replies: &[PriorReviewComment]) -> Option<&PriorReviewComment> {
    replies.iter().rev().take(MAX_DECISION_REPLIES).fold(
        None,
        |best: Option<&PriorReviewComment>, reply| match best {
            Some(best) if best.body.chars().count() >= reply.body.chars().count() => Some(best),
            _ => Some(reply),
        },
    )
}

/// One-line form of a resolved thread beyond the full budget: its title
/// line and the start of its most explanatory human reply.
fn summary_entry(thread: InlineThread, resolution: &Resolution) -> PriorReviewFeedback {
    let title_line = finding_title_line(&thread.root.body).to_owned();
    let (title, _) = truncate_chars(finding_title(&title_line), MAX_LOCATION_CHARS);
    let (path, _) = truncate_chars(&thread.path, MAX_LOCATION_CHARS);
    let (title_line, _) = truncate_chars(&title_line, SUMMARY_TITLE_CHARS);
    let mut comments = vec![PriorReviewComment {
        author: thread.root.author,
        body: title_line,
    }];
    if let Some(reply) = explanatory_reply(&resolution.human_replies) {
        let (body, _) = truncate_chars(reply.body.trim(), SUMMARY_REPLY_CHARS);
        comments.push(PriorReviewComment {
            author: reply.author.clone(),
            body,
        });
    }
    let (location, _) = truncate_chars(&thread.location, SUMMARY_LOCATION_CHARS);
    PriorReviewFeedback {
        location,
        comments,
        resolved: true,
        summarized: true,
        decided: Some(DecidedFinding {
            path,
            line: thread.line,
            anchor: thread.anchor,
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
    options: &FeedbackOptions,
) -> (Vec<PriorReviewFeedback>, FeedbackCuts) {
    let (mut open, mut resolved): (Vec<_>, Vec<_>) =
        threads.into_iter().partition(|thread| !thread.resolved());
    open.sort_by_key(|thread| std::cmp::Reverse(thread.root_id));
    // Resolved threads on files in the current diff first, newer first: the
    // decisions the model is most likely to reopen.
    let in_diff = |thread: &InlineThread| options.changed_paths.contains(&thread.path);
    resolved.sort_by_key(|thread| (!in_diff(thread), std::cmp::Reverse(thread.root_id)));
    if open.len() > MAX_THREADS {
        cuts.dropped_threads += open.len() - MAX_THREADS;
        open.truncate(MAX_THREADS);
    }
    // Decisions share the thread limit with open threads, which come first,
    // and are capped by `max_resolved`; the rest are summarized.
    let resolved_slots = (MAX_THREADS - open.len()).min(options.max_resolved);
    let overflow = resolved.split_off(resolved.len().min(resolved_slots));
    // Open threads first, then resolved ones: the budget is filled in this
    // order, so a resolved thread never displaces an open one.
    let mut candidates: Vec<(u64, InlineThread, PriorReviewFeedback)> = Vec::new();
    for thread in open.into_iter().chain(resolved) {
        let entry = match thread.resolution.clone() {
            Some(resolution) => resolved_entry(thread.clone(), resolution, &mut cuts),
            None => open_entry(thread.clone(), &mut cuts),
        };
        candidates.push((thread.root_id, thread, entry));
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
            summarized: false,
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
    let mut to_summarize = Vec::new();
    for (root_id, thread, entry) in candidates {
        let bytes = feedback_bytes(&entry);
        if bytes <= budget {
            budget -= bytes;
            if entry.resolved {
                cuts.resolved_kept += 1;
            }
            kept.push((root_id, entry));
        } else if entry.resolved {
            to_summarize.push(thread);
        } else {
            cuts.dropped_threads += 1;
        }
    }
    // Resolved threads that did not fit in full come before the overflow;
    // both keep their priority order.
    to_summarize.extend(overflow);
    let mut summary_budget = options.summary_bytes;
    let mut summaries = Vec::new();
    for thread in to_summarize {
        let root_id = thread.root_id;
        let Some(resolution) = thread.resolution.clone() else {
            continue;
        };
        let entry = summary_entry(thread, &resolution);
        let bytes = feedback_bytes(&entry);
        if bytes <= summary_budget {
            summary_budget -= bytes;
            cuts.resolved_summarized += 1;
            cuts.summarized_ids.push(root_id);
            summaries.push((root_id, entry));
        } else {
            cuts.dropped_resolved_threads += 1;
            cuts.dropped_resolved_ids.push(root_id);
        }
    }
    kept.sort_by_key(|(root_id, _)| *root_id);
    summaries.sort_by_key(|(root_id, _)| *root_id);
    cuts.summarized_ids.sort_unstable();
    cuts.dropped_resolved_ids.sort_unstable();
    let mut kept: Vec<_> = kept
        .into_iter()
        .chain(summaries)
        .map(|(_, entry)| entry)
        .collect();
    kept.extend(conversation);
    (kept, cuts)
}

/// Final check that the bounded feedback respects every limit. It must never
/// fail after [`bound_feedback`]; a failure is a bug in the bounds.
pub fn validate_bounds(
    feedback: &[PriorReviewFeedback],
    summary_bytes: usize,
) -> anyhow::Result<()> {
    let mut total = 0usize;
    let mut summaries = 0usize;
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
        if entry.summarized {
            summaries = summaries.saturating_add(feedback_bytes(entry));
        } else {
            total = total.saturating_add(feedback_bytes(entry));
        }
    }
    anyhow::ensure!(
        total <= MAX_FEEDBACK_BYTES,
        "prior feedback exceeds byte limit"
    );
    anyhow::ensure!(
        summaries <= summary_bytes,
        "prior feedback summaries exceed their byte limit"
    );
    Ok(())
}

/// Drop new findings that repeat a decided one: same file, a line within
/// [`DECIDED_LINE_WINDOW`] of the thread's and the same normalized title as a
/// resolved thread with a human reply, when the commented code is provably
/// the same: the host does not mark the thread outdated and the current diff
/// (`files`) holds exactly the anchored lines at the thread's line. A
/// finding or thread without a line or anchor is never dropped, so a distinct
/// occurrence or changed code reaches the review. Returns how many were
/// dropped; the caller logs that count as `suppressed_as_decided`.
pub fn suppress_decided(
    feedback: &[PriorReviewFeedback],
    files: &[cururu_core::ChangedFile],
    review: &mut cururu_core::ReviewResult,
) -> usize {
    let decided: Vec<(&str, u32, String)> = feedback
        .iter()
        .filter(|entry| entry.resolved)
        .filter_map(|entry| entry.decided.as_ref())
        .filter(|decided| !decided.code_changed && !decided.title.trim().is_empty())
        .filter_map(|decided| {
            let line = decided.line?;
            let anchor = decided.anchor.as_ref()?;
            let file = files.iter().find(|file| file.path == decided.path)?;
            anchor_unchanged(anchor, line, &file.patch).then(|| {
                (
                    decided.path.as_str(),
                    line,
                    normalized_title(&decided.title),
                )
            })
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
        // Only the count is logged (by the caller): paths and titles come
        // from the diff and the model and stay out of operational logs.
        !repeated
    });
    before - review.findings.len()
}

/// Drop findings that fall on a thread resolved after the model input was
/// collected (`before`) and before publication (`after`): same file and a
/// line within [`DECIDED_LINE_WINDOW`] of the thread's line or, for a thread
/// without a line, the same normalized title. The model never saw that
/// decision, so it cannot have taken it into account. Returns how many were
/// dropped.
pub fn drop_on_newly_resolved(
    before: &[PriorReviewFeedback],
    after: &[PriorReviewFeedback],
    findings: &mut Vec<cururu_core::ReviewFinding>,
) -> usize {
    let known = |decided: &DecidedFinding| {
        before.iter().any(|entry| {
            entry.resolved
                && entry.decided.as_ref().is_some_and(|old| {
                    old.path == decided.path
                        && old.line == decided.line
                        && old.title == decided.title
                })
        })
    };
    let newly: Vec<&DecidedFinding> = after
        .iter()
        .filter(|entry| entry.resolved)
        .filter_map(|entry| entry.decided.as_ref())
        .filter(|decided| !known(decided))
        .collect();
    if newly.is_empty() {
        return 0;
    }
    let before_count = findings.len();
    findings.retain(|finding| {
        !newly.iter().any(|decided| {
            decided.path == finding.path
                && match (decided.line, finding.line) {
                    (Some(thread), Some(line)) => line.abs_diff(thread) <= DECIDED_LINE_WINDOW,
                    (None, _) => {
                        normalized_title(&decided.title) == normalized_title(&finding.title)
                    }
                    (Some(_), None) => false,
                }
        })
    });
    before_count - findings.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full-budget rules alone: no summaries, as before they existed.
    fn without_summaries() -> FeedbackOptions {
        FeedbackOptions {
            summary_bytes: 0,
            ..FeedbackOptions::new(crate::config::PriorFeedbackResolved::WithHumanReply)
        }
    }

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
            anchor: None,
            location: format!("src/{root_id}.rs (line 1)"),
            root: comment("finding"),
            replies: (0..replies)
                .map(|index| comment(&format!("{index}:{body}")))
                .collect(),
            resolution: None,
        }
    }

    /// A patch adding `src/<id>.rs` with lines `l1`..`l40`, `changed` replaced.
    fn changed_file(id: u64, changed: Option<u32>) -> cururu_core::ChangedFile {
        let lines: Vec<String> = (1..=40)
            .map(|number| {
                let edited = if Some(number) == changed {
                    " edited"
                } else {
                    ""
                };
                format!("+l{number}{edited}\n")
            })
            .collect();
        let patch = format!("@@ -0,0 +1,40 @@\n{}", lines.concat());
        cururu_core::ChangedFile {
            path: format!("src/{id}.rs"),
            patch,
            right_lines: (1..=40).collect(),
        }
    }

    /// A resolved thread on line 5 whose latest human reply is `reply`,
    /// anchored on `l2`..`l5` as written then.
    fn decided(root_id: u64, reply: &str) -> InlineThread {
        InlineThread {
            anchor: anchor_from_hunk("@@ -0,0 +1,5 @@\n+l1\n+l2\n+l3\n+l4\n+l5", None),
            root: comment(&format!(
                "<!-- cururu:finding -->\n\n**MEDIUM**: Finding {root_id}\n\nLong explanation that is not sent."
            )),
            resolution: Some(Resolution {
                human_replies: vec![comment(reply)],
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
            summarized: false,
            decided: None,
        };
        let (feedback, cuts) = bound_feedback(
            threads,
            Some(conversation),
            FeedbackCuts::default(),
            &without_summaries(),
        );
        validate_bounds(&feedback, 0).unwrap();
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
            anchor: None,
            location: "x".repeat(600),
            root: comment(&"ç".repeat(2_500)),
            replies: vec![comment("short")],
            resolution: None,
        });
        let (feedback, cuts) =
            bound_feedback(threads, None, FeedbackCuts::default(), &without_summaries());
        validate_bounds(&feedback, 0).unwrap();
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
            summarized: false,
            decided: None,
        };
        let (feedback, cuts) = bound_feedback(
            Vec::new(),
            Some(conversation),
            FeedbackCuts::default(),
            &without_summaries(),
        );
        validate_bounds(&feedback, 0).unwrap();
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
                anchor: None,
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
        let (feedback, cuts) =
            bound_feedback(threads, None, FeedbackCuts::default(), &without_summaries());
        validate_bounds(&feedback, 0).unwrap();
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
            summarized: false,
            decided: None,
        }];
        assert!(validate_bounds(&feedback, 0).is_err());
        feedback[0].comments[0].body = "ok".into();
        assert!(validate_bounds(&feedback, 0).is_ok());
        feedback[0].location = "x".repeat(MAX_LOCATION_CHARS + 1);
        assert!(validate_bounds(&feedback, 0).is_err());
    }

    #[test]
    fn resolved_threads_beyond_the_budget_are_summarized_in_diff_first() {
        // 60 decisions with a long explanation and a short closing reply,
        // as a long-running PR collects them; files 0..8 are in the diff.
        let explanation = "Kept on purpose: the shared session sets the timeout. ".repeat(8);
        let threads: Vec<InlineThread> = (0..60)
            .map(|index| {
                let mut thread = decided(1000 + index, &explanation);
                thread.path = format!("svc/{}.py", index % 20);
                if let Some(resolution) = &mut thread.resolution {
                    resolution.human_replies.push(comment("resolving"));
                }
                thread
            })
            .collect();
        let options = FeedbackOptions {
            changed_paths: (0..8).map(|index| format!("svc/{index}.py")).collect(),
            ..FeedbackOptions::new(crate::config::PriorFeedbackResolved::WithHumanReply)
        };
        let (feedback, cuts) = bound_feedback(threads, None, FeedbackCuts::default(), &options);
        validate_bounds(&feedback, options.summary_bytes).unwrap();
        // Nothing is dropped: every decision is sent in full or summarized.
        assert_eq!(cuts.dropped_resolved_threads, 0, "{cuts:?}");
        assert_eq!(cuts.resolved_kept + cuts.resolved_summarized, 60);
        assert_eq!(feedback.len(), 60);
        assert_eq!(cuts.summarized_ids.len(), cuts.resolved_summarized);
        // Full entries go to files in the diff first.
        let full: Vec<&PriorReviewFeedback> =
            feedback.iter().filter(|entry| !entry.summarized).collect();
        assert_ne!(full.len(), 0);
        assert!(full.iter().all(|entry| {
            options
                .changed_paths
                .contains(&entry.decided.as_ref().unwrap().path)
        }));
        // A summary keeps the title line and the explanation, not "resolving".
        let summary = feedback.iter().find(|entry| entry.summarized).unwrap();
        assert!(summary.resolved && summary.decided.is_some());
        assert!(summary.comments[0].body.starts_with("**MEDIUM**: Finding"));
        assert!(summary.comments[1].body.starts_with("Kept on purpose"));
        assert!(summary.comments[1].body.chars().count() <= SUMMARY_REPLY_CHARS);
        assert!(!cuts.any(), "summarizing is not reported as a cut");

        // A small summary budget drops the rest, and names them by id.
        let tight = FeedbackOptions {
            summary_bytes: 1_000,
            ..options
        };
        let threads: Vec<InlineThread> = (0..60)
            .map(|index| decided(1000 + index, &explanation))
            .collect();
        let (feedback, cuts) = bound_feedback(threads, None, FeedbackCuts::default(), &tight);
        validate_bounds(&feedback, tight.summary_bytes).unwrap();
        assert!(cuts.dropped_resolved_threads > 0 && cuts.any());
        assert_eq!(
            cuts.dropped_resolved_ids.len(),
            cuts.dropped_resolved_threads
        );
        assert_eq!(
            cuts.resolved_kept + cuts.resolved_summarized + cuts.dropped_resolved_threads,
            60
        );
    }

    #[test]
    fn findings_on_threads_resolved_meanwhile_are_dropped() {
        let before = bound_feedback(
            vec![thread(1, 1, "open")],
            None,
            FeedbackCuts::default(),
            &without_summaries(),
        )
        .0;
        let after = bound_feedback(
            vec![decided(1, "Not applicable.")],
            None,
            FeedbackCuts::default(),
            &without_summaries(),
        )
        .0;
        let mut findings = vec![
            finding("src/1.rs", "Any title"),
            cururu_core::ReviewFinding {
                line: Some(30),
                ..finding("src/1.rs", "Elsewhere in the file")
            },
            finding("src/2.rs", "Another file"),
        ];
        assert_eq!(drop_on_newly_resolved(&before, &after, &mut findings), 1);
        assert_eq!(findings.len(), 2);
        // Unchanged feedback drops nothing.
        assert_eq!(drop_on_newly_resolved(&after, &after, &mut findings), 0);
    }

    #[test]
    fn resolved_threads_are_sent_compact_as_decisions() {
        let long_reply = "ã".repeat(MAX_COMMENT_CHARS + 50);
        let (feedback, cuts) = bound_feedback(
            vec![decided(7, &long_reply), thread(3, 1, "open")],
            None,
            FeedbackCuts::default(),
            &without_summaries(),
        );
        validate_bounds(&feedback, 0).unwrap();
        // Oldest first: the open thread (3), then the decision (7).
        assert!(!feedback[0].resolved && feedback[0].decided.is_none());
        let decision = &feedback[1];
        assert!(decision.resolved);
        assert_eq!(decision.location, "src/7.rs (line 1)");
        // Only the title line and the human reply, truncated.
        assert_eq!(decision.comments.len(), 2);
        assert_eq!(decision.comments[0].body, "**MEDIUM**: Finding 7");
        assert_eq!(decision.comments[1].body.chars().count(), MAX_COMMENT_CHARS);
        assert!(decision.comments[1].body.ends_with(TRUNCATION_MARK));
        assert_eq!(
            decision.decided,
            Some(DecidedFinding {
                path: "src/7.rs".into(),
                line: Some(5),
                anchor: anchor_from_hunk("@@ -0,0 +1,5 @@\n+l1\n+l2\n+l3\n+l4\n+l5", None),
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
        let (feedback, cuts) =
            bound_feedback(threads, None, FeedbackCuts::default(), &without_summaries());
        validate_bounds(&feedback, 0).unwrap();
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
    fn a_decision_keeps_its_latest_human_replies() {
        let mut thread = decided(1, "unused");
        thread.resolution.as_mut().unwrap().human_replies = (0..5)
            .map(|index| comment(&format!("reply {index}")))
            .collect();
        let (feedback, cuts) = bound_feedback(
            vec![thread],
            None,
            FeedbackCuts::default(),
            &without_summaries(),
        );
        let bodies: Vec<_> = feedback[0]
            .comments
            .iter()
            .map(|c| c.body.as_str())
            .collect();
        assert_eq!(
            bodies,
            ["**MEDIUM**: Finding 1", "reply 2", "reply 3", "reply 4"]
        );
        assert_eq!(cuts.dropped_comments, 2);
    }

    #[test]
    fn the_thread_count_limit_cuts_resolved_threads_first() {
        let mut threads: Vec<_> = (1..=18).map(|id| thread(id, 1, "ok")).collect();
        threads.extend((100..=104).map(|id| decided(id, "kept as is")));
        let (feedback, cuts) =
            bound_feedback(threads, None, FeedbackCuts::default(), &without_summaries());
        validate_bounds(&feedback, 0).unwrap();
        assert_eq!(feedback.len(), MAX_THREADS);
        let kept: Vec<_> = feedback
            .iter()
            .filter(|entry| entry.resolved)
            .map(|entry| entry.location.as_str())
            .collect();
        assert_eq!(kept, ["src/103.rs (line 1)", "src/104.rs (line 1)"]);
        assert_eq!(cuts.dropped_resolved_threads, 3);
        assert_eq!(cuts.dropped_resolved_ids, [100, 101, 102]);
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
            &without_summaries(),
        );
        let files = [changed_file(1, None)];
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
        assert_eq!(suppress_decided(&feedback, &files, &mut review), 1);
        let titles: Vec<_> = review.findings.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(titles, ["Another problem", "Finding 1"]);

        // Code changed after the decision: the finding stays.
        let mut changed = feedback.clone();
        changed[0].decided.as_mut().unwrap().code_changed = true;
        let mut review = cururu_core::ReviewResult {
            findings: vec![finding("src/1.rs", "Finding 1")],
            ..review
        };
        assert_eq!(suppress_decided(&changed, &files, &mut review), 0);
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
            &without_summaries(),
        );
        let files = [changed_file(1, None)];
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
        assert_eq!(suppress_decided(&feedback, &files, &mut review), 1);
        let lines: Vec<_> = review.findings.iter().map(|f| f.line).collect();
        assert_eq!(lines, [Some(9), Some(40), None]);

        // A decision without a line suppresses nothing.
        let mut unanchored = feedback.clone();
        unanchored[0].decided.as_mut().unwrap().line = None;
        let mut review = cururu_core::ReviewResult {
            findings: vec![at(Some(5))],
            ..review
        };
        assert_eq!(suppress_decided(&unanchored, &files, &mut review), 0);
    }

    #[test]
    fn changed_code_at_the_same_anchor_is_not_suppressed() {
        // GitHub can keep a thread current when code next to its line
        // changes; the anchored lines must still be the same.
        let (feedback, _) = bound_feedback(
            vec![decided(1, "Not applicable: intentional.")],
            None,
            FeedbackCuts::default(),
            &without_summaries(),
        );
        assert!(!feedback[0].decided.as_ref().unwrap().code_changed);
        let review = || cururu_core::ReviewResult {
            model: "fixture".into(),
            files_reviewed: 1,
            summary: String::new(),
            findings: vec![finding("src/1.rs", "Finding 1")],
        };
        for changed in [3, 5] {
            let mut current = review();
            let files = [changed_file(1, Some(changed))];
            assert_eq!(suppress_decided(&feedback, &files, &mut current), 0);
        }
        // Outside the anchored lines, or unchanged: still the same code.
        let mut current = review();
        assert_eq!(
            suppress_decided(&feedback, &[changed_file(1, Some(20))], &mut current),
            1
        );
        // The file is not in the current diff: nothing to compare with.
        let mut current = review();
        assert_eq!(suppress_decided(&feedback, &[], &mut current), 0);
    }

    #[test]
    fn anchors_come_from_the_new_side_of_the_hunk() {
        let hunk = "@@ -1,4 +1,5 @@\n a\n-b\n+B\n c\n+d\n e";
        // e, d, c, B (removed b is not part of the new side).
        let anchor = anchor_from_hunk(hunk, None).unwrap();
        assert_eq!(anchor.lines, 4);
        assert_eq!(anchor, anchor_of(&["B", "c", "d", "e"]));
        assert_eq!(anchor_from_hunk("@@ -1,2 +1,1 @@\n a\n-b", None), None);
        assert_eq!(anchor_from_hunk("", None), None);
        // A full hunk with context after the commented line (line 5, "e"):
        // the anchor still ends at the commented line.
        let full = "@@ -1,6 +1,8 @@\n a\n-b\n+B\n c\n+d\n e\n f\n+g\n h";
        assert_eq!(anchor_from_hunk(full, Some(5)), Some(anchor.clone()));
        assert_ne!(anchor_from_hunk(full, None), Some(anchor.clone()));
        // A line the hunk does not reach gives no anchor.
        assert_eq!(anchor_from_hunk(full, Some(30)), None);
        let patch = "diff --git a/x b/x\n@@ -1,4 +1,5 @@\n a\n-b\n+B\n c\n+d\n e\n";
        assert!(anchor_unchanged(&anchor, 5, patch));
        assert!(!anchor_unchanged(&anchor, 4, patch));
    }
}
