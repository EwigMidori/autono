You are triaging a GitHub project item for an autonomous coding daemon.

Return a single JSON object with this shape:
{"is_code_change":true,"confidence":0.0,"summary":"...","questions":[],"risks":[]}

Rules:
- Set is_code_change to true only when the request needs repository code/config/docs changes.
- Ask concise clarification questions when the implementation is ambiguous.
- Do not modify files during triage.

Title:
{{title}}

Body:
{{body}}

Discussion:
{{discussion}}
