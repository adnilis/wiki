use crate::sdd::write_plain;
use crate::sdd_types::{CheckInput, SddError, VerificationInput};
use std::path::Path;
use time::macros::format_description;
use time::OffsetDateTime;

pub(crate) fn render_verification(
    dir: &Path,
    verification: &VerificationInput,
    structural_failed: bool,
    structural_issues: &[String],
) -> Result<(), SddError> {
    let mut body = format!(
        "# 验证报告\n\n- Checked at: {}\n- Mode: evidence\n- Result: {}\n- Structural checks: {}\n\n",
        timestamp(),
        verification.result,
        if structural_failed {
            "failed"
        } else {
            "passed"
        }
    );
    append_structural_issues(&mut body, structural_issues);
    body.push_str("\n## Task snapshots\n\n");
    if verification.tasks.is_empty() {
        body.push_str("- 无\n");
    } else {
        for task in &verification.tasks {
            body.push_str(&format!(
                "- {}: {}；Evidence refs: {}\n",
                task.id,
                task.status,
                join_or_none(&task.evidence_refs)
            ));
        }
    }
    body.push_str("\n## Checks\n\n");
    for check in &verification.checks {
        append_check(&mut body, check);
    }
    write_plain(&dir.join("verify.md"), &body)
}

pub(crate) fn render_structural_verification(
    dir: &Path,
    result: &str,
    requirement_count: usize,
    task_count: usize,
    has_tasks: bool,
    structural_failed: bool,
    structural_issues: &[String],
) -> Result<(), SddError> {
    let mut body = format!(
        "# 验证报告\n\n- Checked at: {}\n- Mode: structure\n- Result: {result}\n- Structural checks: {}\n- Checks: {} requirements and {} tasks inspected\n\n",
        timestamp(),
        if structural_failed { "failed" } else { "passed" },
        requirement_count,
        task_count
    );
    body.push_str("- spec/index.md: present\n");
    body.push_str(&format!(
        "- requirements: {} Markdown file(s)\n",
        requirement_count
    ));
    body.push_str(&format!(
        "- tasks: {}\n",
        if has_tasks {
            format!("{} Markdown file(s)", task_count)
        } else {
            "Has tasks: false".into()
        }
    ));
    if structural_issues.is_empty() {
        body.push_str("- result: all structural checks passed\n");
    } else {
        append_structural_issues(&mut body, structural_issues);
    }
    write_plain(&dir.join("verify.md"), &body)
}

fn append_structural_issues(body: &mut String, issues: &[String]) {
    if !issues.is_empty() {
        body.push_str("\n## 结构问题\n\n");
        for issue in issues {
            body.push_str(&format!("- {issue}\n"));
        }
    }
}

fn append_check(body: &mut String, check: &CheckInput) {
    body.push_str(&format!(
        "### {}{}\n\n- Status: {}\n- Command: {}\n- Output: {}\n- Evidence refs: {}\n- Requirement refs: {}\n- Acceptance refs: {}\n- Task refs: {}\n\n",
        check.id.as_deref().map(|id| format!("{}：", id)).unwrap_or_default(),
        check.name,
        check.status,
        check.command.as_deref().unwrap_or("未提供"),
        check.output,
        join_or_none(&check.evidence_refs),
        join_or_none(&check.requirement_refs),
        join_or_none(&check.acceptance_refs),
        join_or_none(&check.task_refs)
    ));
}

fn timestamp() -> String {
    let format = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    OffsetDateTime::now_utc()
        .format(&format)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "无".into()
    } else {
        values.join("；")
    }
}
