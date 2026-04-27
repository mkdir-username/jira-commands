---
description: Create new Jira issues with jirac, including interactive prompts for project, issue type, summary, and custom fields
---

Create a new Jira issue using `jirac`.

Steps:
1. Check that `jirac` is available by running `jirac --version`. If it is missing, tell the user to install it with `cargo install jira-commands`.
2. Extract from the user's request:
   - project key
   - issue type
   - summary
   - any explicit assignee, labels, priority, parent, or other fields
3. Run `jirac issue create` with the fields that are already known.
4. Let `jirac` prompt for any missing required fields.
5. If custom fields are unclear, use `jirac issue fields -p <PROJECT> --issue-type '<TYPE>'` first.
6. Confirm the created issue key clearly.

Examples:
- "create an issue in PROJ" → `jirac issue create -p PROJ`
- "create a bug in PROJ called login page crashes" → `jirac issue create -p PROJ --type Bug --summary 'login page crashes'`
- "buat task baru di PROJ" → `jirac issue create -p PROJ --type Task`
