use time::OffsetDateTime;

use crate::github_types::{
    PullRequestReviewDecisionValue, PullRequestReviewNode, PullRequestReviewStateValue,
};
use crate::workflow::ReviewDecision;

#[derive(Debug, Clone)]
pub(crate) struct ReviewState {
    pub(crate) decision: ReviewDecision,
    pub(crate) review_id: Option<i64>,
    pub(crate) review_body: Option<String>,
}

impl ReviewState {
    pub(crate) fn new(
        decision: ReviewDecision,
        review_id: Option<i64>,
        review_body: Option<String>,
    ) -> Self {
        Self {
            decision,
            review_id,
            review_body,
        }
    }
}

pub(crate) fn review_decision_from_github(
    decision: Option<PullRequestReviewDecisionValue>,
) -> ReviewDecision {
    match decision {
        Some(PullRequestReviewDecisionValue::Approved) => ReviewDecision::Approved,
        Some(PullRequestReviewDecisionValue::ChangesRequested) => ReviewDecision::ChangesRequested,
        _ => ReviewDecision::None,
    }
}

pub(crate) fn review_state_for_decision(
    decision: ReviewDecision,
    reviews: Vec<PullRequestReviewNode>,
) -> ReviewState {
    let Some(wanted_state) = opinionated_review_state(decision) else {
        return ReviewState::new(decision, None, None);
    };
    let latest_review = reviews
        .into_iter()
        .filter(|review| review.state == wanted_state)
        .max_by_key(|review| review.submitted_at.unwrap_or(OffsetDateTime::UNIX_EPOCH));
    let review_id = latest_review
        .as_ref()
        .and_then(|review| review.full_database_id);
    ReviewState::new(
        decision,
        review_id,
        latest_review.and_then(|review| non_empty_review_body(review.body)),
    )
}

fn opinionated_review_state(decision: ReviewDecision) -> Option<PullRequestReviewStateValue> {
    match decision {
        ReviewDecision::Approved => Some(PullRequestReviewStateValue::Approved),
        ReviewDecision::ChangesRequested => Some(PullRequestReviewStateValue::ChangesRequested),
        ReviewDecision::None => None,
    }
}

fn non_empty_review_body(body: String) -> Option<String> {
    let body = body.trim().to_string();
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}
