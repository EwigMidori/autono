use crate::github::{ProjectContent, ProjectItem};
use crate::runner::codex::{CompletionCheckResult, SelfReviewResult};
use crate::workflow::{AutonoMarker, CommentView, ManagedState, TriageResult};

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct CommentComposer {
    blocked_output_limit: usize,
}

impl Default for CommentComposer {
    fn default() -> Self {
        Self {
            blocked_output_limit: 6000,
        }
    }
}

impl CommentComposer {
    pub(crate) fn triage_comment(
        &self,
        item: &ProjectItem,
        result: &TriageResult,
        start_status: &str,
    ) -> (ManagedState, String) {
        if !result.is_code_change {
            return (
                ManagedState::Blocked,
                format!(
                    "{}\n\nI do not think this is a code-change task.\n\nSummary: {}",
                    self.marker(item, ManagedState::Blocked),
                    result.summary
                ),
            );
        }
        if result.needs_clarification() {
            let questions = self.clarification_questions(result);
            return (
                ManagedState::Blocked,
                format!(
                    "{}\n\nI need clarification before implementation.\n\nSummary: {}\n\nQuestions:\n{}",
                    self.marker(item, ManagedState::Blocked),
                    result.summary,
                    questions
                ),
            );
        }
        (
            ManagedState::AwaitingStart,
            format!(
                "{}\n\nI can implement this code-change request.\n\nSummary: {}\n\nMove this Project item to `{}` when it should start.",
                self.marker(item, ManagedState::AwaitingStart),
                result.summary,
                start_status
            ),
        )
    }

    pub(crate) fn discussion_text(&self, comments: &[CommentView]) -> String {
        comments
            .iter()
            .map(|comment| {
                format!(
                    "### {}\n{}\n",
                    if comment.author.is_empty() {
                        "issue body"
                    } else {
                        &comment.author
                    },
                    comment.body
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn blocked_validation_comment(&self, item: &ProjectItem, err: &str) -> String {
        format!(
            "{}\n\nImplementation is blocked after validation retries:\n\n```text\n{}\n```",
            self.marker(item, ManagedState::Blocked),
            self.truncate(err)
        )
    }

    pub(crate) fn blocked_self_review_comment(
        &self,
        item: &ProjectItem,
        review: &SelfReviewResult,
    ) -> String {
        format!(
            "{}\n\nImplementation is blocked after AI self-review.\n\n{}",
            self.marker(item, ManagedState::Blocked),
            self.self_review_body(review)
        )
    }

    pub(crate) fn blocked_completion_comment(
        &self,
        item: &ProjectItem,
        completion: &CompletionCheckResult,
    ) -> String {
        format!(
            "{}\n\nImplementation is blocked after completion check.\n\n{}",
            self.marker(item, ManagedState::Blocked),
            self.completion_body(completion)
        )
    }

    pub(crate) fn no_changes_comment(&self, item: &ProjectItem) -> String {
        format!(
            "{}\n\nImplementation produced no repository changes, so no pull request was opened.",
            self.marker(item, ManagedState::Blocked)
        )
    }

    pub(crate) fn pr_body(&self, item: &ProjectItem, content: &ProjectContent) -> String {
        format!(
            "{}\n\nImplements GitHub Project item `{}`.\n\nSource discussion: {}\n",
            self.marker(item, ManagedState::PrOpen),
            item.id,
            content.url
        )
    }

    pub(crate) fn completion_comment(&self, item: &ProjectItem) -> String {
        format!(
            "{}\n\nThe linked pull request has been merged. Marking this task complete.",
            self.marker(item, ManagedState::Done)
        )
    }

    pub(crate) fn pr_progress_comment(
        &self,
        item: &ProjectItem,
        state: ManagedState,
        branch: &str,
        pr_number: i64,
    ) -> String {
        let marker = AutonoMarker::new(&item.id, state)
            .with_branch(branch)
            .with_pr_number(pr_number)
            .render();
        let action = match state {
            ManagedState::PrOpen => "Opened",
            ManagedState::Reviewing => "Opened draft",
            ManagedState::ReviewPending => "Updated",
            _ => "Tracked",
        };
        format!("{marker}\n\n{action} pull request #{pr_number} for this item.")
    }

    pub(crate) fn self_review_comment(&self, review: &SelfReviewResult) -> String {
        format!(
            "AI self-review result:\n\n{}",
            self.self_review_body(review)
        )
    }

    pub(crate) fn review_ready_comment(&self) -> String {
        "Review Ready: AI self-review passed and this PR is ready for human review.".to_string()
    }

    pub(crate) fn discussion_monitor_comment(
        &self,
        item: &ProjectItem,
        state: ManagedState,
        reply: &str,
    ) -> String {
        format!("{}\n\n{}", self.marker(item, state), self.truncate(reply))
    }

    fn marker(&self, item: &ProjectItem, state: ManagedState) -> String {
        AutonoMarker::new(&item.id, state).render()
    }

    fn clarification_questions(&self, result: &TriageResult) -> String {
        if result.questions.is_empty() {
            "- Please clarify the expected code change.".to_string()
        } else {
            result
                .questions
                .iter()
                .map(|question| format!("- {question}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    fn truncate(&self, input: &str) -> String {
        if input.len() <= self.blocked_output_limit {
            input.to_string()
        } else {
            let end = input
                .char_indices()
                .map(|(index, _)| index)
                .take_while(|index| *index <= self.blocked_output_limit)
                .last()
                .unwrap_or(0);
            format!("{}...", &input[..end])
        }
    }

    fn self_review_body(&self, review: &SelfReviewResult) -> String {
        let findings = self.list_or_none(&review.findings);
        let questions = self.list_or_none(&review.questions);
        self.truncate(&format!(
            "Outcome: {:?}\n\nSummary: {}\n\nFindings:\n{}\n\nQuestions:\n{}",
            review.outcome, review.summary, findings, questions
        ))
    }

    fn completion_body(&self, completion: &CompletionCheckResult) -> String {
        let findings = self.list_or_none(&completion.findings);
        let questions = self.list_or_none(&completion.questions);
        self.truncate(&format!(
            "Outcome: {:?}\n\nSummary: {}\n\nFindings:\n{}\n\nQuestions:\n{}",
            completion.outcome, completion.summary, findings, questions
        ))
    }

    fn list_or_none(&self, items: &[String]) -> String {
        if items.is_empty() {
            "- None".to_string()
        } else {
            items
                .iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}
