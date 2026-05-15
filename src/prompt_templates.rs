pub(crate) const TRIAGE: &str = include_str!("../prompts/triage.md");
pub(crate) const IMPLEMENTATION: &str = include_str!("../prompts/implementation.md");
pub(crate) const IMPLEMENTATION_REPAIR: &str = include_str!("../prompts/implementation_repair.md");
pub(crate) const SELF_REVIEW: &str = include_str!("../prompts/self_review.md");
pub(crate) const SELF_REVIEW_REPAIR: &str = include_str!("../prompts/self_review_repair.md");
pub(crate) const DISCUSSION_MONITOR: &str = include_str!("../prompts/discussion_monitor.md");
pub(crate) const REVIEW_FEEDBACK: &str = include_str!("../prompts/review_feedback.md");
pub(crate) const REVIEW_FEEDBACK_SUMMARY: &str =
    include_str!("../prompts/_partials/review_feedback_summary.md");
pub(crate) const REVIEW_FEEDBACK_THREAD: &str =
    include_str!("../prompts/_partials/review_feedback_thread.md");
pub(crate) const REVIEW_FEEDBACK_COMMENT: &str =
    include_str!("../prompts/_partials/review_feedback_comment.md");
pub(crate) const REVIEW_FEEDBACK_DIFF: &str =
    include_str!("../prompts/_partials/review_feedback_diff.md");

pub(crate) fn render(template: &str, replacements: &[(&str, &str)]) -> String {
    let mut output = template.to_string();
    for (key, value) in replacements {
        output = output.replace(&format!("{{{{{key}}}}}"), value);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_placeholders() {
        let rendered = render("Hello {{name}}", &[("name", "world")]);

        assert_eq!(rendered, "Hello world");
    }
}
