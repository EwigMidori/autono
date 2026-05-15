use std::collections::HashMap;

use crate::config::TargetConfig;
use crate::error::Result;
use crate::github::{GitHub, ReviewThread};
use crate::prompt_templates::{
    render as render_template, REVIEW_FEEDBACK, REVIEW_FEEDBACK_COMMENT, REVIEW_FEEDBACK_DIFF,
    REVIEW_FEEDBACK_SUMMARY, REVIEW_FEEDBACK_THREAD,
};
use crate::workflow::ManagedState;

const REVIEW_FEEDBACK_LIMIT: usize = 120_000;
const REVIEW_COMMENT_BODY_LIMIT: usize = 12_000;
const REVIEW_COMMENT_DIFF_LIMIT: usize = 24_000;

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct ReviewFeedbackComposer {
    output_limit: usize,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub(crate) struct FeedbackRequest<'a> {
    target: &'a TargetConfig,
    state: ManagedState,
    pr_number: Option<i64>,
    review_body: Option<&'a str>,
}

impl Default for ReviewFeedbackComposer {
    fn default() -> Self {
        Self {
            output_limit: REVIEW_FEEDBACK_LIMIT,
        }
    }
}

impl ReviewFeedbackComposer {
    pub(crate) async fn trusted_feedback<G: GitHub>(
        &self,
        github: &G,
        target: &TargetConfig,
        pr_number: i64,
    ) -> Result<String> {
        let threads = github.list_review_threads(target, pr_number).await?;
        let threads = filter_review_threads(threads);
        let mut permissions = HashMap::<String, bool>::new();
        let mut trusted = Vec::new();
        for thread in threads {
            let mut thread_comments = Vec::new();
            for comment in thread.comments {
                let allowed = match permissions.get(&comment.author) {
                    Some(allowed) => *allowed,
                    None => {
                        let allowed = github
                            .user_can_administer_or_write(target, &comment.author)
                            .await?;
                        permissions.insert(comment.author.clone(), allowed);
                        allowed
                    }
                };
                if allowed {
                    thread_comments.push(comment);
                }
            }
            if !thread_comments.is_empty() {
                trusted.push(ReviewThread {
                    comments: thread_comments,
                    ..thread
                });
            }
        }
        Ok(self.render(&trusted, None))
    }

    pub(crate) async fn trusted_feedback_for_state<G: GitHub>(
        &self,
        github: &G,
        request: FeedbackRequest<'_>,
    ) -> Result<String> {
        if request.state != ManagedState::ReviewPending {
            return Ok(String::new());
        }
        let Some(pr_number) = request.pr_number else {
            return Ok(String::new());
        };
        let review_feedback = self
            .trusted_feedback(github, request.target, pr_number)
            .await?;
        Ok(self.render_review_context(&review_feedback, request.review_body))
    }

    pub(crate) fn prepend_to_discussion(&self, review_feedback: &str, discussion: &str) -> String {
        if review_feedback.is_empty() {
            discussion.to_string()
        } else {
            format!("{review_feedback}\n\nIssue and PR discussion:\n{discussion}")
        }
    }

    fn render_review_context(&self, review_feedback: &str, review_body: Option<&str>) -> String {
        let review_body_block = review_body
            .filter(|body| !body.trim().is_empty())
            .map(|body| {
                let review_body = truncate_end(body, REVIEW_COMMENT_BODY_LIMIT);
                let rendered =
                    render_template(REVIEW_FEEDBACK_SUMMARY, &[("review_body", &review_body)]);
                format!("{rendered}\n\n")
            })
            .unwrap_or_default();
        let output = render_template(
            REVIEW_FEEDBACK,
            &[
                ("review_body_block", &review_body_block),
                ("thread_sections", review_feedback),
            ],
        );
        truncate_end(&output, self.output_limit)
    }

    fn render(&self, comments: &[ReviewThread], review_body: Option<&str>) -> String {
        if comments.is_empty() && review_body.is_none() {
            return String::new();
        }
        let review_feedback = comments
            .iter()
            .map(|thread| self.render_thread(thread))
            .collect::<Vec<_>>()
            .join("\n");
        self.render_review_context(&review_feedback, review_body)
    }

    fn render_thread(&self, thread: &ReviewThread) -> String {
        let comments = thread
            .comments
            .iter()
            .map(|comment| self.render_comment(comment))
            .collect::<Vec<_>>()
            .join("\n");
        render_template(
            REVIEW_FEEDBACK_THREAD,
            &[
                ("id", &thread.id),
                ("path", &thread.path),
                ("line", &thread.line_label()),
                (
                    "status",
                    if thread.is_resolved {
                        "resolved"
                    } else {
                        "open"
                    },
                ),
                ("comments", &comments),
            ],
        )
    }

    fn render_comment(&self, comment: &crate::github::ReviewThreadComment) -> String {
        let body = truncate_end(&comment.body, REVIEW_COMMENT_BODY_LIMIT);
        let diff_hunk_block = if comment.diff_hunk.trim().is_empty() {
            String::new()
        } else {
            let diff_hunk = truncate_end(&comment.diff_hunk, REVIEW_COMMENT_DIFF_LIMIT);
            render_template(REVIEW_FEEDBACK_DIFF, &[("diff_hunk", &diff_hunk)])
        };
        render_template(
            REVIEW_FEEDBACK_COMMENT,
            &[
                ("author", &comment.author),
                ("url", &comment.html_url),
                ("body", &body),
                ("diff_hunk_block", &diff_hunk_block),
            ],
        )
    }
}

impl<'a> FeedbackRequest<'a> {
    pub(crate) fn new(
        target: &'a TargetConfig,
        state: ManagedState,
        pr_number: Option<i64>,
        review_body: Option<&'a str>,
    ) -> Self {
        Self {
            target,
            state,
            pr_number,
            review_body,
        }
    }
}

fn filter_review_threads(threads: Vec<ReviewThread>) -> Vec<ReviewThread> {
    threads
        .into_iter()
        .filter(|thread| !thread.is_resolved && !thread.is_outdated)
        .collect()
}

fn truncate_end(input: &str, limit: usize) -> String {
    if input.len() <= limit {
        return input.to_string();
    }
    let note = format!("\n[truncated {} bytes]", input.len() - limit);
    let body_limit = limit.saturating_sub(note.len());
    let end = input
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= body_limit)
        .last()
        .unwrap_or(0);
    format!("{}{}", &input[..end], note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{ReviewThread, ReviewThreadComment};

    fn comment(author: &str, body: &str) -> ReviewThreadComment {
        ReviewThreadComment {
            author: author.to_string(),
            body: body.to_string(),
            diff_hunk: "@@ -1 +1 @@".to_string(),
            html_url: "https://github.test/review-comment".to_string(),
        }
    }

    fn thread(id: &str, is_resolved: bool, is_outdated: bool) -> ReviewThread {
        ReviewThread {
            id: id.to_string(),
            is_resolved,
            is_outdated,
            path: "src/lib.rs".to_string(),
            line: Some(42),
            original_line: None,
            comments: vec![comment("maintainer", "Fix this line")],
        }
    }

    #[test]
    fn formatter_includes_file_line_and_diff() {
        let rendered =
            ReviewFeedbackComposer::default().render(&[thread("T1", false, false)], None);

        assert!(rendered.contains("src/lib.rs"));
        assert!(rendered.contains("Line: 42"));
        assert!(rendered.contains("Fix this line"));
        assert!(rendered.contains("@@ -1 +1 @@"));
        assert!(rendered.contains("Reply with `gh api graphql`"));
    }

    #[test]
    fn filters_outdated_and_resolved_threads() {
        let comments = filter_review_threads(vec![
            thread("T1", false, false),
            thread("T2", true, false),
            thread("T3", false, true),
        ]);

        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].id, "T1");
    }
}
