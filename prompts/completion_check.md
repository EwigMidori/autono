Check whether the current repository worktree fully completes this GitHub task.

Task summary:
{{summary}}

Discussion and review context:
{{discussion}}

Expected validation commands:
{{tests}}

Rules:
- Inspect the current worktree and implemented diff against the task discussion.
- Check for unfinished work, placeholders, missing files, partial implementations, and validation gaps.
- Do not modify files during this check.
- Return a single JSON object with this shape:
{"outcome":"complete","summary":"...","findings":[],"questions":[]}
- Set outcome to "complete" only when the implementation is complete enough to finalize and self-review.
- Set outcome to "needs_work" when the implementation can be completed without more user input.
- Set outcome to "blocked" when more user information is required or the work cannot proceed safely.
