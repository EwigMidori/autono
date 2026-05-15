Review the current repository worktree before this pull request is sent to human reviewers.

Task summary:
{{summary}}

Discussion and review context:
{{discussion}}

Expected validation commands:
{{tests}}

Rules:
- Inspect the implemented diff against the base branch and the task discussion.
- Check for incomplete implementation, missing edge cases, low-quality code, and missing validation.
- Do not modify files during review.
- Return a single JSON object with this shape:
{"outcome":"ready","summary":"...","findings":[],"questions":[]}
- Set outcome to "ready" only when the implementation is complete enough for human review.
- Set outcome to "needs_fix" when the implementation can be fixed without more user input.
- Set outcome to "blocked" when more user information is required or the work cannot proceed safely.
