---
description: View full Jira issue details with jirac, including description, attachments, and other issue metadata
---

View full Jira issue details using `jirac`.

Steps:
1. Check that `jirac` is available by running `jirac --version`. If it is missing, tell the user to install it with `cargo install jira-commands`.
2. Extract the issue key from the request.
3. Run `jirac issue view <ISSUE-KEY>`.
4. Show the output clearly and use it to answer follow-up questions.

Examples:
- "view PROJ-123" → `jirac issue view PROJ-123`
- "show me the details of PROJ-456" → `jirac issue view PROJ-456`
