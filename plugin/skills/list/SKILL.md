---
description: List Jira issues with jirac by project, assignee, or custom JQL, including the default current-user flow
---

List Jira issues using `jirac`.

Steps:
1. Check that `jirac` is available by running `jirac --version`. If it is missing, tell the user to install it with `cargo install jira-commands`.
2. Determine the query from the request:
   - project key
   - explicit JQL
   - otherwise use the default assigned-to-current-user behavior
3. Run `jirac issue list` with the right flags.
4. Present the results clearly.

Notes:
- `jirac issue list` without flags defaults to `assignee = currentUser()`.
- `jirac issue list -p PROJ` limits to one project.
- `jirac issue list --jql '<expression>'` gives full control.

Examples:
- "list my issues" → `jirac issue list`
- "list issues in PROJ" → `jirac issue list -p PROJ`
- "list open bugs in PROJ" → `jirac issue list --jql 'project = PROJ AND issuetype = Bug AND status != Done'`
