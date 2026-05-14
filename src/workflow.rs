use std::str::FromStr;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use time::OffsetDateTime;

use crate::config::WorkflowConfig;

const MARKER_PREFIX: &str = "<!-- autono:";
const MARKER_SUFFIX: &str = "-->";

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "PascalCase")]
pub enum ManagedState {
    Detected,
    Triaged,
    AwaitingStart,
    Working,
    PrOpen,
    ReviewPending,
    Done,
    Blocked,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    None,
    ChangesRequested,
    Approved,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkflowAction {
    Ignore,
    Triage,
    WaitForStart,
    StartWork,
    WaitForReview,
    ApplyReviewFeedback,
    WaitForMerge,
    Complete,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct ItemView {
    pub(crate) managed_state: Option<ManagedState>,
    pub(crate) project_status: Option<String>,
    pub(crate) has_admin_mention: bool,
    pub(crate) has_new_human_comment: bool,
    pub(crate) has_pr: bool,
    pub(crate) pr_merged: bool,
    pub(crate) review_decision: ReviewDecision,
    pub(crate) has_unhandled_review: bool,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct WorkflowPolicy {
    workflow: WorkflowConfig,
}

impl WorkflowPolicy {
    pub(crate) fn new(workflow: WorkflowConfig) -> Self {
        Self { workflow }
    }

    pub(crate) fn decide_next_action(&self, view: &ItemView) -> WorkflowAction {
        if view.pr_merged {
            return WorkflowAction::Complete;
        }

        match view.managed_state {
            None => {
                if view.has_admin_mention {
                    WorkflowAction::Triage
                } else {
                    WorkflowAction::Ignore
                }
            }
            Some(ManagedState::Detected | ManagedState::Triaged | ManagedState::AwaitingStart) => {
                if self.project_status_matches(&view.project_status, &self.workflow.start_status) {
                    WorkflowAction::StartWork
                } else {
                    WorkflowAction::WaitForStart
                }
            }
            Some(ManagedState::Working) => {
                if view.has_pr {
                    WorkflowAction::WaitForReview
                } else {
                    WorkflowAction::StartWork
                }
            }
            Some(ManagedState::PrOpen) => match view.review_decision {
                ReviewDecision::ChangesRequested => WorkflowAction::ApplyReviewFeedback,
                ReviewDecision::Approved => WorkflowAction::WaitForMerge,
                ReviewDecision::None => WorkflowAction::WaitForReview,
            },
            Some(ManagedState::ReviewPending) => match view.review_decision {
                ReviewDecision::ChangesRequested if view.has_unhandled_review => {
                    WorkflowAction::ApplyReviewFeedback
                }
                ReviewDecision::ChangesRequested | ReviewDecision::None => {
                    WorkflowAction::WaitForReview
                }
                ReviewDecision::Approved => WorkflowAction::WaitForMerge,
            },
            Some(ManagedState::Done) => WorkflowAction::Ignore,
            Some(ManagedState::Blocked) => {
                if view.has_new_human_comment {
                    WorkflowAction::Triage
                } else {
                    WorkflowAction::WaitForStart
                }
            }
        }
    }

    fn project_status_matches(&self, current: &Option<String>, wanted: &str) -> bool {
        current
            .as_deref()
            .map(|status| status.eq_ignore_ascii_case(wanted))
            .unwrap_or(false)
    }
}

impl From<WorkflowConfig> for WorkflowPolicy {
    fn from(workflow: WorkflowConfig) -> Self {
        Self::new(workflow)
    }
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct BotMentionPolicy {
    mention: String,
}

impl BotMentionPolicy {
    pub(crate) fn new(bot_login: &str) -> Self {
        let login = bot_login.trim_start_matches('@').to_ascii_lowercase();
        Self {
            mention: format!("@{login}"),
        }
    }

    pub(crate) fn contains_mention(&self, body: &str) -> bool {
        body.to_ascii_lowercase()
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '@'))
            .any(|token| token == self.mention)
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutonoMarker {
    item_id: String,
    state: ManagedState,
}

impl AutonoMarker {
    pub(crate) fn new(item_id: &str, state: ManagedState) -> Self {
        Self {
            item_id: item_id.to_string(),
            state,
        }
    }

    pub(crate) fn render(&self) -> String {
        format!(
            "{MARKER_PREFIX} item={} state={} {MARKER_SUFFIX}",
            self.item_id, self.state
        )
    }

    pub(crate) fn state(&self) -> ManagedState {
        self.state
    }

    pub(crate) fn find_in(body: &str) -> Option<Self> {
        let start = body.find(MARKER_PREFIX)?;
        let rest = &body[start..];
        let end = rest.find(MARKER_SUFFIX)?;
        let marker = &rest[..end];
        let item_id = marker
            .split_whitespace()
            .find_map(|part| part.strip_prefix("item="))?;
        let state = marker
            .split_whitespace()
            .find_map(|part| part.strip_prefix("state="))
            .and_then(|state| ManagedState::from_str(state).ok())?;
        Some(Self::new(item_id, state))
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageResult {
    pub is_code_change: bool,
    #[serde(default)]
    pub confidence: f32,
    pub summary: String,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
}

impl TriageResult {
    pub fn needs_clarification(&self) -> bool {
        !self.questions.is_empty() || self.confidence < 0.60
    }
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CommentView {
    pub id: i64,
    pub author: String,
    pub body: String,
    pub created_at: OffsetDateTime,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct CommentThread {
    comments: Vec<CommentView>,
}

impl CommentThread {
    pub(crate) fn new(comments: Vec<CommentView>) -> Self {
        Self { comments }
    }

    pub(crate) fn comments(&self) -> &[CommentView] {
        &self.comments
    }

    pub(crate) fn latest_admin_mention(
        &self,
        policy: &BotMentionPolicy,
        is_admin: impl Fn(&str) -> bool,
    ) -> Option<AdminMention> {
        self.comments
            .iter()
            .filter(|comment| policy.contains_mention(&comment.body))
            .filter(|comment| is_admin(&comment.author))
            .max_by_key(|comment| comment.created_at)
            .map(|_| AdminMention)
    }

    pub(crate) fn latest_marker_state(&self) -> Option<MarkerView> {
        self.comments
            .iter()
            .filter_map(|comment| {
                AutonoMarker::find_in(&comment.body).map(|marker| MarkerView {
                    comment_id: comment.id,
                    state: marker.state(),
                })
            })
            .max_by_key(|marker| marker.comment_id)
    }

    pub(crate) fn latest_human_comment_id(&self, bot_login: &str) -> Option<i64> {
        self.comments
            .iter()
            .filter(|comment| comment.author != bot_login)
            .map(|comment| comment.id)
            .max()
    }

    pub(crate) fn has_new_human_comment_since(
        &self,
        last_seen: Option<i64>,
        bot_login: &str,
    ) -> bool {
        let latest = self.latest_human_comment_id(bot_login);
        latest
            .zip(last_seen)
            .map(|(latest, last)| latest > last)
            .unwrap_or(latest.is_some())
    }
}

impl From<Vec<CommentView>> for CommentThread {
    fn from(comments: Vec<CommentView>) -> Self {
        Self::new(comments)
    }
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct AdminMention;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MarkerView {
    pub(crate) comment_id: i64,
    pub(crate) state: ManagedState,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BOT_LOGIN: &str = "mtshit";

    fn workflow() -> WorkflowConfig {
        WorkflowConfig {
            status_field: "Status".to_string(),
            triaged_status: "Triaged".to_string(),
            start_status: "In Progress".to_string(),
            review_status: "In Review".to_string(),
            done_status: "Done".to_string(),
            blocked_status: "Blocked".to_string(),
        }
    }

    #[test]
    fn mention_parser_requires_exact_login_token() {
        let policy = BotMentionPolicy::new(TEST_BOT_LOGIN);
        assert!(policy.contains_mention(&format!("please @{} take this", TEST_BOT_LOGIN)));
        assert!(BotMentionPolicy::new(&format!("@{}", TEST_BOT_LOGIN))
            .contains_mention("please @MtShit."));
        assert!(!policy.contains_mention(&format!("@{}2 should not match", TEST_BOT_LOGIN)));
        assert!(!policy.contains_mention(&format!("{} without at", TEST_BOT_LOGIN)));
    }

    #[test]
    fn marker_parser_recovers_state() {
        let marker = AutonoMarker::new("I_1", ManagedState::AwaitingStart).render();
        assert_eq!(
            AutonoMarker::find_in(&marker).map(|marker| marker.state()),
            Some(ManagedState::AwaitingStart)
        );
        assert_eq!(AutonoMarker::find_in("plain comment"), None);
    }

    #[test]
    fn unmanaged_item_requires_admin_mention() {
        let view = ItemView {
            managed_state: None,
            project_status: Some("Todo".to_string()),
            has_admin_mention: false,
            has_new_human_comment: false,
            has_pr: false,
            pr_merged: false,
            review_decision: ReviewDecision::None,
            has_unhandled_review: false,
        };
        let policy = WorkflowPolicy::new(workflow());
        assert_eq!(policy.decide_next_action(&view), WorkflowAction::Ignore);
    }

    #[test]
    fn awaiting_start_only_starts_on_configured_status() {
        let mut view = ItemView {
            managed_state: Some(ManagedState::AwaitingStart),
            project_status: Some("Triaged".to_string()),
            has_admin_mention: true,
            has_new_human_comment: false,
            has_pr: false,
            pr_merged: false,
            review_decision: ReviewDecision::None,
            has_unhandled_review: false,
        };
        let policy = WorkflowPolicy::new(workflow());
        assert_eq!(
            policy.decide_next_action(&view),
            WorkflowAction::WaitForStart
        );
        view.project_status = Some("In Progress".to_string());
        assert_eq!(policy.decide_next_action(&view), WorkflowAction::StartWork);
    }

    #[test]
    fn pr_review_drives_followup_state() {
        let mut view = ItemView {
            managed_state: Some(ManagedState::PrOpen),
            project_status: Some("In Review".to_string()),
            has_admin_mention: true,
            has_new_human_comment: false,
            has_pr: true,
            pr_merged: false,
            review_decision: ReviewDecision::ChangesRequested,
            has_unhandled_review: true,
        };
        let policy = WorkflowPolicy::new(workflow());
        assert_eq!(
            policy.decide_next_action(&view),
            WorkflowAction::ApplyReviewFeedback
        );
        view.review_decision = ReviewDecision::Approved;
        assert_eq!(
            policy.decide_next_action(&view),
            WorkflowAction::WaitForMerge
        );
        view.pr_merged = true;
        assert_eq!(policy.decide_next_action(&view), WorkflowAction::Complete);
    }

    #[test]
    fn review_pending_does_not_repeat_same_changes_requested_review() {
        let mut view = ItemView {
            managed_state: Some(ManagedState::ReviewPending),
            project_status: Some("In Review".to_string()),
            has_admin_mention: true,
            has_new_human_comment: false,
            has_pr: true,
            pr_merged: false,
            review_decision: ReviewDecision::ChangesRequested,
            has_unhandled_review: false,
        };
        let policy = WorkflowPolicy::new(workflow());

        assert_eq!(
            policy.decide_next_action(&view),
            WorkflowAction::WaitForReview
        );
        view.has_unhandled_review = true;
        assert_eq!(
            policy.decide_next_action(&view),
            WorkflowAction::ApplyReviewFeedback
        );
    }
}
