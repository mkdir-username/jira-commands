//! Domain-level grouping for issue lists.
//!
//! Maps each Jira issue to one of five buckets aligned with the team's
//! mental model:
//!
//! | Group       | Triggers |
//! |-------------|----------|
//! | Бизнес      | key starts with `PAYDAY-`, or type `Development` |
//! | Техника     | key starts with `DS-`, or type `Dev Web Task` |
//! | Уязвимости  | key starts with `SEC-`, or type starts with `Уязвим` |
//! | Прочее      | fallthrough |
//! | Контейнер   | `ZPTECH-*` or type `Technical task` — quarterly aggregator label |
//!
//! Контейнер sits at the very end: the team-lead creates a `ZPTECH-*` ticket
//! per quarter that just aggregates approved tech items — it is not real
//! work itself, so we hide it behind everything actionable.
//!
//! Order is meaningful — `IssueCategory` derives `Ord` so `.sort()` lays
//! groups in priority order: Бизнес → Техника → Уязвимости → Прочее → Контейнер.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum IssueCategory {
    Business,
    Tech,
    Vulnerability,
    Other,
    Container,
}

impl IssueCategory {
    pub(crate) fn label(self) -> &'static str {
        match self {
            IssueCategory::Business => "Бизнес",
            IssueCategory::Tech => "Техника",
            IssueCategory::Vulnerability => "Уязвимости",
            IssueCategory::Other => "Прочее",
            IssueCategory::Container => "Контейнер (квартал)",
        }
    }
}

pub(crate) fn categorize_issue(key: &str, issue_type: &str) -> IssueCategory {
    let prefix = key.split_once('-').map(|(p, _)| p).unwrap_or("");

    // Quarterly container — checked first so ZPTECH never falls into Tech.
    if prefix == "ZPTECH" || issue_type == "Technical task" {
        return IssueCategory::Container;
    }
    // Vulnerability has next-highest specificity.
    if prefix == "SEC" || issue_type.starts_with("Уязвим") {
        return IssueCategory::Vulnerability;
    }
    if prefix == "PAYDAY" {
        return IssueCategory::Business;
    }
    if prefix == "DS" {
        return IssueCategory::Tech;
    }
    if issue_type == "Development" {
        return IssueCategory::Business;
    }
    if issue_type == "Dev Web Task" {
        return IssueCategory::Tech;
    }
    IssueCategory::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payday_is_business_regardless_of_type() {
        assert_eq!(
            categorize_issue("PAYDAY-1544", "Development"),
            IssueCategory::Business
        );
        assert_eq!(
            categorize_issue("PAYDAY-1450", "Task"),
            IssueCategory::Business
        );
    }

    #[test]
    fn ds_is_tech() {
        assert_eq!(
            categorize_issue("DS-16194", "Dev Web Task"),
            IssueCategory::Tech
        );
    }

    #[test]
    fn zptech_and_technical_task_are_container() {
        assert_eq!(
            categorize_issue("ZPTECH-4080", "Technical task"),
            IssueCategory::Container
        );
        assert_eq!(
            categorize_issue("FOO-9", "Technical task"),
            IssueCategory::Container
        );
    }

    #[test]
    fn sec_and_uyazvim_are_vulnerability() {
        assert_eq!(
            categorize_issue("SEC-730258", "Уязвимость"),
            IssueCategory::Vulnerability
        );
        assert_eq!(
            categorize_issue("FOO-1", "Уязвимость средней критичности"),
            IssueCategory::Vulnerability
        );
    }

    #[test]
    fn type_fallback_for_unknown_projects() {
        assert_eq!(
            categorize_issue("FOO-1", "Development"),
            IssueCategory::Business
        );
        assert_eq!(
            categorize_issue("BAR-2", "Dev Web Task"),
            IssueCategory::Tech
        );
    }

    #[test]
    fn dbank_bug_is_other() {
        assert_eq!(categorize_issue("DBANK-365", "Bug"), IssueCategory::Other);
    }

    #[test]
    fn category_order_business_first_container_last() {
        let mut cats = vec![
            IssueCategory::Container,
            IssueCategory::Other,
            IssueCategory::Vulnerability,
            IssueCategory::Tech,
            IssueCategory::Business,
        ];
        cats.sort();
        assert_eq!(
            cats,
            vec![
                IssueCategory::Business,
                IssueCategory::Tech,
                IssueCategory::Vulnerability,
                IssueCategory::Other,
                IssueCategory::Container,
            ]
        );
    }
}
