---
description: Build or run Jira JQL queries with jirac, either directly from a query string or through the interactive JQL helper
---

Build and execute JQL using `jirac`.

Steps:
1. Check that `jirac` is available by running `jirac --version`. If it is missing, tell the user to install it with `cargo install jira-commands`.
2. If the user already has a JQL expression, run it directly with `jirac issue list --jql '<expression>'`.
3. If the user wants help composing a query, run `jirac issue jql`.
4. Explain the resulting issues or the generated query clearly.

Examples:
- "run JQL: project = PROJ AND status = 'In Progress'" → `jirac issue list --jql 'project = PROJ AND status = "In Progress"'`
- "help me build a JQL query" → `jirac issue jql`
- "find all bugs assigned to me in PROJ" → `jirac issue list --jql 'project = PROJ AND issuetype = Bug AND assignee = currentUser()'`
