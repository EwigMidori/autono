use serde::Deserialize;

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionOutcome {
    Complete,
    NeedsWork,
    Blocked,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CompletionCheckResult {
    pub outcome: CompletionOutcome,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelfReviewOutcome {
    Ready,
    NeedsFix,
    Blocked,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SelfReviewResult {
    pub outcome: SelfReviewOutcome,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ImplementationResult {
    pub summary: String,
    pub tests_run: Vec<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
pub struct DiscussionReplyDecision {
    pub should_reply: bool,
    #[serde(default)]
    pub reply: String,
}
