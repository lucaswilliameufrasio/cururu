//! Deterministic bounds for prior review feedback sent to the model.
//!
//! Earlier discussion is useful context but must never stop a review: every
//! limit here cuts the input instead of failing. The PR conversation after the
//! latest summary has priority over inline threads, and newer threads have
//! priority over older ones.

use crate::scm::{PriorReviewComment, PriorReviewFeedback};

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
/// Total bytes of locations and comment bodies.
pub const MAX_FEEDBACK_BYTES: usize = 12_000;
/// Appended to a body or location cut at its character limit.
pub const TRUNCATION_MARK: &str = "…[truncado]";

/// What the bounds removed, for the operator notice.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FeedbackCuts {
    /// Resolved threads skipped because they were already decided.
    pub resolved_threads: usize,
    /// Threads beyond [`MAX_THREADS`] or the byte budget.
    pub dropped_threads: usize,
    /// Thread replies and conversation comments beyond their count limit or
    /// the byte budget.
    pub dropped_comments: usize,
    /// Bodies or locations cut at their character limit.
    pub truncated_comments: usize,
}

impl FeedbackCuts {
    pub const fn any(&self) -> bool {
        self.dropped_threads > 0 || self.dropped_comments > 0 || self.truncated_comments > 0
    }

    pub fn summary(&self) -> String {
        format!(
            "prior review feedback was cut to fit its limits: {} thread(s) and {} comment(s) dropped, {} comment(s) truncated, {} resolved thread(s) skipped",
            self.dropped_threads,
            self.dropped_comments,
            self.truncated_comments,
            self.resolved_threads
        )
    }
}

/// One inline thread: its root comment id (newer threads have larger ids),
/// location, root comment and replies in chronological order.
#[derive(Debug, Clone)]
pub struct InlineThread {
    pub root_id: u64,
    pub location: String,
    pub root: PriorReviewComment,
    pub replies: Vec<PriorReviewComment>,
}

/// Cut `value` to at most `max_chars` characters on a character boundary,
/// ending with [`TRUNCATION_MARK`] when it was cut.
pub fn truncate_chars(value: &str, max_chars: usize) -> (String, bool) {
    if value.chars().count() <= max_chars {
        return (value.to_owned(), false);
    }
    let keep = max_chars.saturating_sub(TRUNCATION_MARK.chars().count());
    let mut cut: String = value.chars().take(keep).collect();
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
        cuts.truncated_comments += 1;
    }
    location
}

/// Bytes counted against [`MAX_FEEDBACK_BYTES`].
pub fn feedback_bytes(feedback: &PriorReviewFeedback) -> usize {
    feedback.location.len()
        + feedback
            .comments
            .iter()
            .map(|comment| comment.body.len())
            .sum::<usize>()
}

/// Apply every limit: keep the newest [`MAX_THREADS`] threads and the latest
/// replies and conversation comments, truncate long bodies, then fill the
/// byte budget with the conversation first and threads from newest to oldest,
/// dropping whole threads that do not fit. Threads are returned oldest first,
/// followed by the conversation.
pub fn bound_feedback(
    mut threads: Vec<InlineThread>,
    conversation: Option<PriorReviewFeedback>,
    mut cuts: FeedbackCuts,
) -> (Vec<PriorReviewFeedback>, FeedbackCuts) {
    threads.sort_by_key(|thread| std::cmp::Reverse(thread.root_id));
    if threads.len() > MAX_THREADS {
        cuts.dropped_threads += threads.len() - MAX_THREADS;
        threads.truncate(MAX_THREADS);
    }
    let threads: Vec<PriorReviewFeedback> = threads
        .into_iter()
        .map(|thread| {
            let mut replies = thread.replies;
            if replies.len() > MAX_REPLIES_PER_THREAD {
                let excess = replies.len() - MAX_REPLIES_PER_THREAD;
                cuts.dropped_comments += excess;
                replies.drain(..excess);
            }
            let mut comments = Vec::with_capacity(replies.len() + 1);
            comments.push(bounded_comment(thread.root, &mut cuts));
            comments.extend(
                replies
                    .into_iter()
                    .map(|reply| bounded_comment(reply, &mut cuts)),
            );
            PriorReviewFeedback {
                location: bounded_location(&thread.location, &mut cuts),
                comments,
            }
        })
        .collect();

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
    for thread in threads {
        let bytes = feedback_bytes(&thread);
        if bytes <= budget {
            budget -= bytes;
            kept.push(thread);
        } else {
            cuts.dropped_threads += 1;
        }
    }
    kept.reverse();
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
        total = total.saturating_add(feedback_bytes(entry));
    }
    anyhow::ensure!(
        total <= MAX_FEEDBACK_BYTES,
        "prior feedback exceeds byte limit"
    );
    Ok(())
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
            location: format!("src/{root_id}.rs (line 1)"),
            root: comment("finding"),
            replies: (0..replies)
                .map(|index| comment(&format!("{index}:{body}")))
                .collect(),
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
    }

    #[test]
    fn budget_keeps_the_conversation_and_the_newest_threads() {
        let threads: Vec<_> = (1..=10)
            .map(|id| thread(id, 1, &"x".repeat(1_900)))
            .collect();
        let conversation = PriorReviewFeedback {
            location: "PR conversation".into(),
            comments: vec![comment("please also check the retry")],
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
            location: "x".repeat(600),
            root: comment(&"ç".repeat(2_500)),
            replies: vec![comment("short")],
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
        assert_eq!(cuts.truncated_comments, 2);
    }

    #[test]
    fn an_oversized_conversation_keeps_its_latest_comments() {
        let conversation = PriorReviewFeedback {
            location: "PR conversation".into(),
            comments: (0..12)
                .map(|index| comment(&format!("{index}{}", "é".repeat(1_990))))
                .collect(),
        };
        let (feedback, cuts) =
            bound_feedback(Vec::new(), Some(conversation), FeedbackCuts::default());
        validate_bounds(&feedback).unwrap();
        let kept = &feedback[0].comments;
        assert!(kept.last().unwrap().body.starts_with("11"));
        assert_eq!(cuts.dropped_comments, 12 - kept.len());
    }

    #[test]
    fn validation_still_reports_out_of_bounds_input() {
        let mut feedback = vec![PriorReviewFeedback {
            location: "a".into(),
            comments: vec![comment(&"é".repeat(MAX_FEEDBACK_BYTES))],
        }];
        assert!(validate_bounds(&feedback).is_err());
        feedback[0].comments[0].body = "ok".into();
        assert!(validate_bounds(&feedback).is_ok());
        feedback[0].location = "x".repeat(MAX_LOCATION_CHARS + 1);
        assert!(validate_bounds(&feedback).is_err());
    }
}
