---
description: Manage Jira worklogs with jirac, including list, add, and delete flows for issue time tracking
---

Manage Jira worklogs using `jirac`.

Steps:
1. Check that `jirac` is available by running `jirac --version`. If it is missing, tell the user to install it with `cargo install jira-commands`.
2. Determine the requested action.

For listing worklogs:
- extract the issue key
- run `jirac issue worklog list <ISSUE-KEY>`

For adding a worklog:
- extract the issue key and time spent
- optionally extract the comment
- run `jirac issue worklog add <ISSUE-KEY> --time '<TIME>' [--comment '<COMMENT>']`

For deleting a worklog:
- extract the issue key and worklog ID
- run `jirac issue worklog delete <ISSUE-KEY> --id <WORKLOG-ID>`

3. Show the result clearly.

Examples:
- "show worklogs for PROJ-123" → `jirac issue worklog list PROJ-123`
- "log 2 hours on PROJ-123" → `jirac issue worklog add PROJ-123 --time '2h'`
- "log 1h 30m on PROJ-456 for fixing login bug" → `jirac issue worklog add PROJ-456 --time '1h 30m' --comment 'fixing login bug'`
- "delete worklog 10234 on PROJ-123" → `jirac issue worklog delete PROJ-123 --id 10234`
