# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## Fork context — corp-adapt branch

**This checkout is `mkdir-username/jira-commands` on branch `corp-adapt`** — a fork of upstream `mulhamna/jira-commands` adapted for Alfa-Bank Jira Data Center (`jira.moscow.alfaintra.net`).

Active divergences from upstream (do NOT regress when editing core code):

| Commit | What | Why it matters |
|--------|------|----------------|
| `73c893a` | `crates/jira-core/Cargo.toml`: reqwest features `rustls-tls` → `rustls-tls-native-roots` | Embedded webpki-roots lacks corp Alfa Root CA. Native picks up macOS keychain / Linux ca-certificates store at runtime. Reverting → `invalid peer certificate: UnknownIssuer` |
| `df160a0` | `crates/jira-core/src/client.rs::search_issues` branches by `api_version` | Cloud's `POST /rest/api/3/search/jql` does NOT exist on DC; v2 must use legacy `POST /rest/api/2/search` with `startAt` pagination. **Upstream's "Endpoint rules" table below is Cloud-only — DC code paths must stay v2-aware.** |
| `4e4005f` | `crates/jira/src/cli/issue.rs::truncate` + inline summary cut use `chars().take()` | Slicing `&str` by byte panics on Cyrillic / CJK (`byte index N is not a char boundary`). Any new truncation must use chars, not bytes. |
| `778572e` | `crates/jira-core/src/adf.rs::adf_to_text` passes `Value::String` through | Cloud returns rich-text fields as ADF JSON trees; DC returns plain strings (wiki markup). The renderer must handle both. CRLF normalised to LF. |
| `50e18b6`, `155cea9` | TUI/CLI column widths + `column_spacing(3)` + minimal default `visible_columns` | Reasonable defaults for narrow terminals — power users still expand via `C` keybind. |
| `849356b`, `496159c` | Default JQL: `resolution = Unresolved ORDER BY issuetype ASC, updated DESC` | `jirac issue list` and TUI hide finished work by default; `jirac issue view <KEY>` still fetches any issue regardless of status. |
| `95bb3d0`, `ae98b6b`, `216aea5` | `crates/jira/src/categorize.rs` (shared module) + grouping in CLI list and TUI | See "Domain grouping" below. |

**Single-purpose modules added by the fork:**
- `crates/jira/src/categorize.rs` — `IssueCategory` enum + `categorize_issue(key, type)` used by both `cli/issue.rs::print_grouped_issues` and `tui/app.rs::set_issues`. **Both paths must call into this module — do not inline categorisation logic anywhere else.**

**Sync with upstream:**
```bash
git fetch upstream
git rebase upstream/main
# resolve conflicts: prefer fork patches when they touch the rows above
```

---

## Domain grouping (corp-adapt)

`categorize_issue(key, type)` → 5-bucket enum. Order is the display order:

| # | Category | Triggers |
|---|----------|----------|
| 1 | Бизнес      | key `PAYDAY-*` OR type `Development` |
| 2 | Техника     | key `DS-*` OR type `Dev Web Task` |
| 3 | Уязвимости  | key `SEC-*` OR type starting with `Уязвим` (Russian DC) |
| 4 | Прочее      | fallthrough |
| 5 | Контейнер   | key `ZPTECH-*` OR type `Technical task` (quarterly aggregator label — checked first to short-circuit Tech) |

Update the categorize tests in the same module if you touch the function — `cargo test -p jira-commands categorize` runs them in isolation.

---

## Claude rules — MUST follow

Claude may:
- Create and edit files on the filesystem
- Run `cargo` commands for build/test/check
- Run `git` commands for read and write operations when explicitly requested by the repo owner, including `git add`, `git commit`, and `git push`
- Do not manually bump versions, edit generated changelogs, create/push tags, or rewrite git history unless explicitly requested by the repo owner

### TASK.md — work checklist

`TASK.md` (gitignored) is Claude's work checklist.
1. Read it at the start of every new session
2. Update `[ ]` → `[x]` immediately after task is done and smoke test passes
3. If missing, recreate from conversation context or ask repo owner

---

## Project overview

Rust CLI for Atlassian Jira (`jirac` binary). Focus: full custom field via dynamic introspection, attachment upload/download (image auto-download + compression for agent context), Jira REST API v3, interactive TUI (ratatui), single binary.

### Workspace structure

```
crates/
├── jira-core/                  # PUBLIC LIBRARY (crates.io: "jira-core")
│   └── src/
│       ├── adf.rs              # Atlassian Document Format parser
│       ├── auth.rs             # auth + multi-profile (Cloud + Data Center)
│       ├── client.rs           # JiraClient — REST API surface
│       ├── config.rs           # JiraConfig loader (figment: toml + env)
│       ├── error.rs            # JiraError + Result alias
│       ├── field_cache.rs      # custom-field id ↔ name resolution cache
│       └── model/              # attachment, comment, field, issue, sprint, worklog
├── jira/                       # BINARY (crates.io: "jira-commands", binary: jirac)
│   └── src/
│       ├── main.rs
│       ├── datetime.rs         # datetime helpers
│       ├── cli/                # clap commands: api, auth, issue, plan
│       └── tui/                # ratatui TUI — modular split
│           ├── app.rs          # event loop coordinator
│           ├── column.rs       # column layout
│           ├── keys.rs         # keybinding map
│           ├── mode.rs         # app mode state
│           ├── panel.rs        # split-pane logic
│           ├── picker.rs       # native pickers
│           ├── prefs.rs        # prefs overlay
│           ├── prompts.rs      # inline prompts
│           ├── render.rs       # frame render
│           └── theme.rs        # color theme
└── jira-mcp/                   # MCP SERVER (crates.io: "jira-mcp", binary: jirac-mcp)
    └── src/
        ├── main.rs
        ├── lib.rs
        ├── app.rs              # MCP app wiring
        ├── server.rs           # rmcp server impl
        ├── models.rs           # MCP tool I/O types
        └── error.rs
plugin/
├── .claude-plugin/             # Claude Code plugin metadata (plugin.json)
└── skills/                     # 12 skills: api, attach, bulk-transition, comment, create-issue, fields, jql, list-issues, transition, update-issue, view-issue, worklog
```

### Crate responsibilities

- **`jira-core`** — public API: `JiraClient`, model types (split under `model/`), ADF parser, auth (Cloud + Data Center multi-profile), `FieldCache` for custom-field resolution, error types. Library dependency.
- **`jira/`** — clap commands (`cli/{api,auth,issue,plan}.rs`), modular TUI (ratatui + crossterm, 10 submodules under `tui/`), interactive prompts (inquire), datetime helpers. Binary: `jirac`.
- **`jira-mcp/`** — MCP server via `rmcp`, split into `app.rs` (wiring) + `server.rs` (impl) + `models.rs` (I/O types). Exposes `jira-core` as MCP tools for LLM clients. Binary: `jirac-mcp`.

---

## Jira API — implementation rules

### Endpoint rules

| Use                                         | Do NOT use                                           |
| ------------------------------------------- | ---------------------------------------------------- |
| `GET/POST /rest/api/3/search/jql`           | `/rest/api/3/search` (dead since Oct 2025)           |
| `POST /rest/api/3/search/approximate-count` | `/rest/api/3/fieldconfiguration*` (removed Jul 2026) |
| `GET /rest/api/3/projects/fields`           |                                                      |
| `GET /rest/api/3/priorityscheme`            |                                                      |

### Implementation principles

- **Pagination**: cursor-based (`next_page_token`), not offset (`startAt`). Max 500 iterations as safeguard.
- **Rate limiting**: handle 429 with `Retry-After` header, retry after delay.
- **Field resolution**: always runtime via API, never hardcode `customfield_*`.
- **Async tasks**: submit → poll → complete pattern for heavy operations (archive, bulk ops).
- **Tier detection**: check `server_info.is_premium()` before using premium features (Plans API).
- **Base URL**: Platform API (`/rest/api/3`) vs Agile API (`/rest/agile/1.0`) — don't mix, use client methods.

---

## ECCF custom fields (Alfa DC)

Alfa's "Extended Context Custom Fields" plugin (`ru.alfabank.atlassian.jira.eccf`) owns required fields like Delivery component (`customfield_59170`, `eccf-single-select-type`). Its handler does `getOption(Integer.parseInt(optionId))` — so `fields:{cf:"JS"}` → 500 and `fields:{cf:{"value":...}}` → 400 "Operation value must be a string". **Write only via `update.set` with the numeric option-id as a string, never the `fields` path:**

```bash
jirac issue create ... --set customfield_59170=625      # 625 = option-id, not "JS"
```

Emits `{"update":{"customfield_59170":[{"set":"625"}]}}`. Multi-select → `[{"set":["id1","id2"]}]`. In bulk/batch manifests use the `"update"` key. `--set` → `parse_set_flags` (`cli/issue.rs`) → `create_issue_v2` (`jira-core/client.rs`).

Resolve a display value → option-id via the plugin (type codes are Gson `@SerializedName` numbers — PROJECT=`"1"`, ISSUE_TYPE=`"2"`, **not names**):

```
GET /rest/eccf/1.0/context/select/options?fieldId=<num>&params=<urlenc [{"type":"1","valueIds":[projectId]},{"type":"2","valueIds":[issueTypeId]}]>
```

Resolve a display value → option-id without hand-rolling that request: `jirac eccf options --field 59170 -p PAYDAY -t Development`.

Source reverse-engineered from Bitbucket repo **`JIRA/extended-context-custom-fields`** (found via `/bb-search`). Full API map + live IDs: project memory `eccf-fields.md`.

### Sub-task recipe (PAYDAY)

The sub-task issue type in PAYDAY is **`Development`**, not `Sub-task` — read an existing sub-task before assuming a type. Working command:

```bash
jirac issue create -p PAYDAY -t Development --parent PAYDAY-1831 \
  -s '[Web Mobile] - 9.0 — Экран ошибки' \
  --components Frontend --set customfield_59170=625 --assignee me --no-custom-fields
```

Jira can persist an issue and still answer 500 — after any error check `jirac issue list --jql 'parent = <KEY>'` before retrying, or the retry duplicates it in production.

### Users are deployment-shaped

DC has no `accountId` — `/rest/api/2/myself` returns `name` + `key`. `client.rs::user_ref_field` picks the field per deployment (`accountId` on Cloud, `name` on DC), `resolve_assignee_ref` returns the whole user object, and `search_users` sends `username=` on DC vs `query=` on Cloud. New code touching `fields.assignee` (or any user field) goes through `resolve_assignee_ref` — hardcoding `{"accountId": ...}` compiles fine and fails only against a live DC.

---

## Smoke test

Claude runs this before reporting to repo owner. Fix until all green.

```bash
cargo fmt --all -- --check && \
cargo clippy --all-targets --all-features -- -D warnings && \
cargo test --all && \
cargo build --all
```

---

## Release flow — release-please (automated)

**Never manually bump versions, update CHANGELOG, or push tags.**

### How it works

1. Push commits to `main` with **Conventional Commits**
2. release-please creates/updates a Release PR (version bump + CHANGELOG)
3. Merge Release PR → pushes tag → `release.yml` triggers build + publish

### Conventional Commits

Format: `<type>(<scope>): <description>` — in English.

| Type                              | Bump       |
| --------------------------------- | ---------- |
| `feat:`                           | MINOR      |
| `fix:`, `perf:`, `refactor:`      | PATCH      |
| `feat!:` / `BREAKING CHANGE:`     | MAJOR      |
| `chore:`, `docs:`, `ci:`, `test:` | No release |

### crates.io publish order

`jira-core` → sparse index ready → `jira-mcp` → `jira-commands`. Never publish manually.

### CI workflows

See `.github/workflows/` for details — actual files are source of truth.
- **ci.yml**: fmt + clippy + test + build (matrix: ubuntu/macos/windows)
- **security.yml**: `cargo audit`
- **release-please.yml**: auto version bump + CHANGELOG + tag
- **release-tag.yml**: build binaries + publish to crates.io (trigger: tag `v*`)
- **release-recover.yml**: recovery workflow for failed releases
- **clawhub-publish-jirac.yml**: publish to ClawHub plugin marketplace
- **pr-automerge.yml**: auto-merge release-please PRs
- **winget-submit.yml**: submit to Windows Package Manager

### Release strategy

Uses `simple` release type with a `VERSION` file at repo root. Cargo.toml files updated via `generic` updater in `release-please-config.json`. Plugin version in `plugin/.claude-plugin/plugin.json` is separate and NOT auto-bumped by release-please.

### Plugin marketplace

When adding/changing skills, update `plugin/skills/<skill>/SKILL.md` and the table in README.
