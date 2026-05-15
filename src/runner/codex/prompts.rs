use std::path::PathBuf;

use super::results::{CompletionCheckResult, SelfReviewResult};
use crate::prompt_templates::{
    render as render_template, COMPLETION_CHECK, COMPLETION_REPAIR, DISCUSSION_MONITOR,
    IMPLEMENTATION, IMPLEMENTATION_REPAIR, SELF_REVIEW, SELF_REVIEW_REPAIR, TRIAGE,
};

const PROMPT_SECTION_LIMIT: usize = 200_000;

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct TriagePrompt {
    title: String,
    body: String,
    comments: String,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct ImplementationPrompt {
    summary: String,
    discussion: String,
    tests: Vec<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct DiscussionPrompt {
    title: String,
    body: String,
    discussion: String,
    state: String,
    base_branch: String,
    start_status: String,
    readonly_checkout: PathBuf,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct DiscussionPromptContext {
    pub state: String,
    pub base_branch: String,
    pub start_status: String,
    pub readonly_checkout: PathBuf,
}

impl TriagePrompt {
    pub(crate) fn new(title: &str, body: &str, comments: &str) -> Self {
        Self {
            title: title.to_string(),
            body: body.to_string(),
            comments: comments.to_string(),
        }
    }

    pub(crate) fn render(&self) -> String {
        let title = truncate_end(&self.title, PROMPT_SECTION_LIMIT);
        let body = truncate_end(&self.body, PROMPT_SECTION_LIMIT);
        let comments = truncate_end(&self.comments, PROMPT_SECTION_LIMIT);
        render_template(
            TRIAGE,
            &[
                ("title", &title),
                ("body", &body),
                ("discussion", &comments),
            ],
        )
    }
}

impl ImplementationPrompt {
    pub(crate) fn new(summary: &str, discussion: &str, tests: &[String]) -> Self {
        Self {
            summary: summary.to_string(),
            discussion: discussion.to_string(),
            tests: tests.to_vec(),
        }
    }

    pub(crate) fn render(&self) -> String {
        let summary = truncate_end(&self.summary, PROMPT_SECTION_LIMIT);
        let discussion = truncate_end(&self.discussion, PROMPT_SECTION_LIMIT);
        let tests = if self.tests.is_empty() {
            "(none configured)".to_string()
        } else {
            truncate_end(&self.tests.join("\n"), PROMPT_SECTION_LIMIT)
        };
        render_template(
            IMPLEMENTATION,
            &[
                ("summary", &summary),
                ("discussion", &discussion),
                ("tests", &tests),
            ],
        )
    }

    pub(crate) fn render_repair(&self, validation_output: &str) -> String {
        let validation_output = truncate_start(validation_output, VALIDATION_OUTPUT_PROMPT_LIMIT);
        let base_prompt = self.render();
        render_template(
            IMPLEMENTATION_REPAIR,
            &[
                ("base_prompt", &base_prompt),
                ("validation_output", &validation_output),
            ],
        )
    }

    pub(crate) fn render_completion_check(&self) -> String {
        let summary = truncate_end(&self.summary, PROMPT_SECTION_LIMIT);
        let discussion = truncate_end(&self.discussion, PROMPT_SECTION_LIMIT);
        let tests = if self.tests.is_empty() {
            "(none configured)".to_string()
        } else {
            truncate_end(&self.tests.join("\n"), PROMPT_SECTION_LIMIT)
        };
        render_template(
            COMPLETION_CHECK,
            &[
                ("summary", &summary),
                ("discussion", &discussion),
                ("tests", &tests),
            ],
        )
    }

    pub(crate) fn render_completion_repair(&self, result: &CompletionCheckResult) -> String {
        let base_prompt = self.render();
        let completion_result = result.to_prompt_text();
        let completion_result = truncate_end(&completion_result, PROMPT_SECTION_LIMIT);
        render_template(
            COMPLETION_REPAIR,
            &[
                ("base_prompt", &base_prompt),
                ("completion_result", &completion_result),
            ],
        )
    }

    pub(crate) fn render_self_review(&self) -> String {
        let summary = truncate_end(&self.summary, PROMPT_SECTION_LIMIT);
        let discussion = truncate_end(&self.discussion, PROMPT_SECTION_LIMIT);
        let tests = if self.tests.is_empty() {
            "(none configured)".to_string()
        } else {
            truncate_end(&self.tests.join("\n"), PROMPT_SECTION_LIMIT)
        };
        render_template(
            SELF_REVIEW,
            &[
                ("summary", &summary),
                ("discussion", &discussion),
                ("tests", &tests),
            ],
        )
    }

    pub(crate) fn render_self_review_repair(&self, review_result: &SelfReviewResult) -> String {
        let base_prompt = self.render();
        let review_result = review_result.to_prompt_text();
        let review_result = truncate_end(&review_result, PROMPT_SECTION_LIMIT);
        render_template(
            SELF_REVIEW_REPAIR,
            &[
                ("base_prompt", &base_prompt),
                ("review_result", &review_result),
            ],
        )
    }
}

impl DiscussionPrompt {
    pub(crate) fn new(
        title: &str,
        body: &str,
        discussion: &str,
        context: DiscussionPromptContext,
    ) -> Self {
        Self {
            title: title.to_string(),
            body: body.to_string(),
            discussion: discussion.to_string(),
            state: context.state,
            base_branch: context.base_branch,
            start_status: context.start_status,
            readonly_checkout: context.readonly_checkout,
        }
    }

    pub(crate) fn render(&self) -> String {
        let title = truncate_end(&self.title, PROMPT_SECTION_LIMIT);
        let body = truncate_end(&self.body, PROMPT_SECTION_LIMIT);
        let discussion = truncate_end(&self.discussion, PROMPT_SECTION_LIMIT);
        let readonly_checkout = self.readonly_checkout.display().to_string();
        render_template(
            DISCUSSION_MONITOR,
            &[
                ("readonly_checkout", &readonly_checkout),
                ("base_branch", &self.base_branch),
                ("state", &self.state),
                ("start_status", &self.start_status),
                ("title", &title),
                ("body", &body),
                ("discussion", &discussion),
            ],
        )
    }
}

impl CompletionCheckResult {
    pub(crate) fn to_prompt_text(&self) -> String {
        prompt_result_text(
            &format!("{:?}", self.outcome),
            &self.summary,
            &self.findings,
            &self.questions,
        )
    }
}

impl SelfReviewResult {
    pub(crate) fn to_prompt_text(&self) -> String {
        prompt_result_text(
            &format!("{:?}", self.outcome),
            &self.summary,
            &self.findings,
            &self.questions,
        )
    }
}

pub(crate) const VALIDATION_OUTPUT_PROMPT_LIMIT: usize = 80_000;

fn prompt_result_text(
    outcome: &str,
    summary: &str,
    findings: &[String],
    questions: &[String],
) -> String {
    let findings = list_or_none(findings);
    let questions = list_or_none(questions);
    format!(
        "Outcome: {outcome}\nSummary: {summary}\nFindings:\n{findings}\nQuestions:\n{questions}"
    )
}

fn list_or_none(items: &[String]) -> String {
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

fn truncate_start(input: &str, limit: usize) -> String {
    if input.len() <= limit {
        return input.to_string();
    }
    let note = format!("[truncated {} bytes]\n", input.len() - limit);
    let body_limit = limit.saturating_sub(note.len());
    let start = input
        .char_indices()
        .map(|(index, _)| index)
        .find(|index| input.len() - *index <= body_limit)
        .unwrap_or(input.len());
    format!("{}{}", note, &input[start..])
}
