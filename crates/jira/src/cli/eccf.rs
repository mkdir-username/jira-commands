use anyhow::{Context, Result};
use clap::Subcommand;
use jira_core::JiraClient;

#[derive(Debug, Subcommand)]
pub enum EccfCommand {
    /// List selectable options (id + title) for an ECCF select custom field.
    ///
    /// ECCF (Extended Context Custom Fields) select fields reject the plain
    /// `fields` write path — they only accept an `update` set operation with the
    /// numeric option id. Discover that id here, then create with:
    ///   jirac issue create ... --set customfield_<field>=<id>
    Options {
        /// Custom field id — `59170` or `customfield_59170`
        #[arg(short, long)]
        field: String,
        /// Project key (e.g. PAYDAY) or numeric project id
        #[arg(short, long)]
        project: String,
        /// Issue type name (e.g. Development) or numeric id
        #[arg(short = 't', long = "issue-type")]
        issue_type: String,
    },
}

pub async fn handle(cmd: EccfCommand, client: JiraClient) -> Result<()> {
    match cmd {
        EccfCommand::Options {
            field,
            project,
            issue_type,
        } => options(client, &field, &project, &issue_type).await,
    }
}

async fn resolve_project_id(client: &JiraClient, project: &str) -> Result<String> {
    if !project.is_empty() && project.chars().all(|c| c.is_ascii_digit()) {
        return Ok(project.to_string());
    }
    client
        .get_project_id(project)
        .await
        .with_context(|| format!("Failed to resolve project id for '{project}'"))
}

async fn resolve_issue_type_id(
    client: &JiraClient,
    project: &str,
    issue_type: &str,
) -> Result<String> {
    if !issue_type.is_empty() && issue_type.chars().all(|c| c.is_ascii_digit()) {
        return Ok(issue_type.to_string());
    }
    let types = client
        .get_issue_types(project)
        .await
        .context("Failed to fetch issue types")?;
    types
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(issue_type))
        .map(|t| t.id.clone())
        .with_context(|| format!("Issue type '{issue_type}' not found in {project}"))
}

async fn options(client: JiraClient, field: &str, project: &str, issue_type: &str) -> Result<()> {
    let project_id = resolve_project_id(&client, project).await?;
    let issue_type_id = resolve_issue_type_id(&client, project, issue_type).await?;
    let field_id = field.trim_start_matches("customfield_");

    let opts = client
        .eccf_select_options(field_id, &project_id, &issue_type_id)
        .await
        .context("Failed to fetch ECCF options")?;

    if opts.is_empty() {
        println!("No options for field {field} (project {project}, type {issue_type}).");
        return Ok(());
    }

    println!("{:<10} TITLE", "ID");
    println!("{}", "─".repeat(40));
    for o in &opts {
        let flag = if o.is_disabled {
            "  (disabled)"
        } else if o.is_required {
            "  (required)"
        } else {
            ""
        };
        println!("{:<10} {}{}", o.id, o.title, flag);
    }
    Ok(())
}
