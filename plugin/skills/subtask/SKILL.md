---
description: Create a sub-task under a parent Jira issue with jirac, including sub-task type discovery, required plugin fields (ECCF option-ids), and post-create verification
---

Create a sub-task under an existing parent issue using `jirac`.

Steps:
1. Check that `jirac` is available by running `jirac --version`. If it is missing, tell the user to install it with `cargo install jira-commands`.
2. Read the parent: `jirac issue view <PARENT>`. Take the project key from it.
3. Discover the sub-task issue type — do not assume it is called `Sub-task`. List existing sub-tasks with `jirac issue list --jql 'project = <PROJ> AND issuetype in subTaskIssueTypes()'` and reuse the type you see there (on Alfa's PAYDAY project it is `Development`). Data Center rejects `parent is not EMPTY` with a 400 — use `subTaskIssueTypes()` or `parent = <KEY>`.
4. Copy the shape of an existing sub-task: `jirac issue view <EXISTING>` shows the components and required custom fields the project actually uses.
5. Find the required fields: `jirac issue fields -p <PROJ> --issue-type '<TYPE>' --required-only`. Fields owned by the ECCF plugin (e.g. Delivery component, `customfield_59170`) take a numeric option-id, not the display value — resolve it with `jirac eccf options --field <NUM> -p <PROJ> -t '<TYPE>'`.
6. Create it: `jirac issue create -p <PROJ> -t '<TYPE>' --parent <PARENT> -s '<summary>' --components '<C>' --set customfield_XXXXX=<option-id> --assignee me --no-custom-fields`
7. Verify and report the key: `jirac issue list --jql 'parent = <PARENT>'`.

Notes:
- ECCF fields are writable only through `--set` (an `update` operation carrying the option-id as a string). The plain `fields` path fails: `--field cf=JS` → 500, `--field 'cf={"value":"JS"}'` → 400 "Operation value must be a string".
- `--set` also covers multi-select: `--set customfield_XXXXX='["625","626"]'`.
- On a 500 or 400, run the verify command from step 7 before retrying — Jira can persist the issue and still return an error, and a blind retry then duplicates it.
- Jira is production. Do not create throwaway issues to probe the schema; steps 3-5 are all read-only.

Examples:
- "create a sub-task under PAYDAY-1831 for the error screen" → `jirac issue create -p PAYDAY -t Development --parent PAYDAY-1831 -s '[Web Mobile] - 9.0 — Экран ошибки' --components Frontend --set customfield_59170=625 --assignee me --no-custom-fields`
- "what sub-task type does this project use?" → `jirac issue list --jql 'project = PAYDAY AND issuetype in subTaskIssueTypes()' --limit 5`
- "which option-id is JS?" → `jirac eccf options --field 59170 -p PAYDAY -t Development`
