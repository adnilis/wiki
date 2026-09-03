use std::fs;
use std::process::{Command, Output};
use tempfile::tempdir;

fn wiki(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wiki"))
        .args(args)
        .output()
        .expect("wiki command should run")
}

fn wiki_owned(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wiki"))
        .args(args)
        .output()
        .expect("wiki command should run")
}

fn create_change(root: &str, title: &str) -> String {
    let output = wiki_owned(&[
        "sdd".into(),
        "new".into(),
        title.into(),
        "--path".into(),
        root.into(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_yaml::Value = serde_yaml::from_slice(&output.stdout).unwrap();
    response["change"]["id"].as_str().unwrap().to_string()
}

fn write_markdown_spec(root: &std::path::Path, change_id: &str) {
    let change_dir = root.join("sdd/changes").join(change_id);
    fs::create_dir_all(change_dir.join("spec/requirements")).unwrap();
    fs::write(
        change_dir.join("spec/index.md"),
        "# 规格\n\n## 摘要\n\n支持可审计的订单取消。\n\n## 证据链\n\n- E-001 [CODE] src/order/cancel.rs：当前入口按状态判断。\n\n## 探索记录\n\n- 当前状态：OrderService 处理取消。\n\n## 决策记录\n\n- DEC-001：保留现有接口。\n\n## 方案比较\n\n- ALT-001：事件驱动释放库存（已选）。\n\n## 影响分析矩阵\n\n| 维度 | 对象 | 变更 | 兼容性 | 风险 | 缓解措施 | 证据 |\n|---|---|---|---|---|---|---|\n| 调用链 | OrderService | 增加幂等分支 | 兼容 | 中 | 幂等键 | E-001 |\n\n## 接口契约与外部对齐\n\n- POST /orders/{id}/cancel：保留并扩展错误码。\n\n## 外部接口依赖与版本依据\n\n- order-api-contract.md v2：已确认。\n\n## 非代码改动与人工对齐\n\n- config/order.cancel.v2：发布前配置。\n\n## 高危操作及人工复核\n\n- retry：指数退避和最大次数。\n\n## 风险分级与发布护栏\n\n- 风险等级：高；灰度、回滚、监控、告警、审批均已记录。\n\n## 测试策略\n\n- 单元、集成、回归测试均覆盖。\n\n## 需求\n\n- [[requirements/REQ-001]] 用户可以取消可取消状态的订单。\n",
    )
    .unwrap();
    fs::write(
        change_dir.join("spec/requirements/REQ-001.md"),
        "# REQ-001：用户可以取消可取消状态的订单\n\n- Priority: must\n- Status: proposed\n- Source refs: E-001\n- Dependencies: 无\n\n## 验收条件\n\n### AC-001\n\n- Given：订单处于待支付状态\n- When：用户提交取消请求\n- Then：订单进入已取消状态且重复请求幂等\n",
    )
    .unwrap();
    fs::create_dir_all(change_dir.join("tasks")).unwrap();
    fs::write(
        change_dir.join("tasks/index.md"),
        "# 任务\n\n- Has tasks: false\n",
    )
    .unwrap();
}

fn write_tasked_spec(root: &std::path::Path, change_id: &str, with_coverage: bool) {
    write_markdown_spec(root, change_id);
    let change_dir = root.join("sdd/changes").join(change_id);
    let coverage = if with_coverage {
        "| REQ-001 | AC-001 | TASK-001 | done | E-001 |\n"
    } else {
        ""
    };
    fs::write(
        change_dir.join("tasks/index.md"),
        format!(
            "# 任务\n\n- Has tasks: true ｜ 总数 1 ｜ 已完成 1/1\n\n## 看板\n\n| 状态 | 任务 | 覆盖 | 阻塞/备注 |\n|---|---|---|---|\n| done | TASK-001 | REQ-001 / AC-001 | - |\n\n## 执行顺序\n\n- 批次 1：TASK-001\n\n## 覆盖矩阵\n\n| REQ | 验收 | 任务 | 状态 | 证据 |\n|---|---|---|---|---|\n{coverage}"
        ),
    )
    .unwrap();
    fs::write(
        change_dir.join("tasks/TASK-001.md"),
        "# TASK-001：实现取消能力\n\n- Priority: must ｜ Status: done ｜ Type: code\n- Requirement refs: REQ-001\n- Acceptance refs: AC-001\n- Evidence refs: E-001\n- Depends on: 无\n- Blocked reason: 无\n\n## 范围\n\n实现订单取消。\n\n## 完成定义\n\n- [x] 代码与测试均完成\n\n## 复核与回退\n\n- Review: 已复核\n- Rollback: 回退上一版本\n\n## Log\n\n- 2026-09-02 todo → done：完成实现与测试\n",
    )
    .unwrap();
}

#[test]
fn sdd_defaults_to_yaml_without_a_format_flag() {
    let root = tempdir().unwrap();
    let root = root.path().to_str().unwrap();
    let output = wiki(&["sdd", "new", "default yaml", "--path", root]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("schemaVersion: wiki.sdd/v1\n"));
    let response: serde_yaml::Value = serde_yaml::from_str(&stdout).unwrap();
    assert_eq!(
        response["actionRequired"]["kind"].as_str(),
        Some("write_spec")
    );
    assert!(response["actionRequired"]["path"]
        .as_str()
        .unwrap()
        .contains("sdd/changes/"));
    assert!(response["actionRequired"]["instructions"]
        .as_sequence()
        .unwrap()
        .iter()
        .any(|value| value.as_str().unwrap().contains("spec/index.md")));
    assert!(response["actionRequired"]["instructions"]
        .as_sequence()
        .unwrap()
        .iter()
        .any(|value| value.as_str().unwrap().contains("tasks/index.md")));
    assert!(response["actionRequired"].get("submit").is_none());
    assert!(response["actionRequired"].get("responseSchema").is_none());
    let change_path = std::path::Path::new(root).join(response["change"]["path"].as_str().unwrap());
    assert!(!change_path.join("spec").exists());
    assert!(!change_path.join("tasks").exists());
}

#[test]
fn sdd_json_flag_selects_json_output() {
    let root = tempdir().unwrap();
    let root = root.path().to_str().unwrap();
    let output = wiki(&["sdd", "new", "json output", "--path", root, "--json"]);

    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schemaVersion"], "wiki.sdd/v1");
    assert_eq!(response["ok"], true);
    assert_eq!(response["actionRequired"]["kind"], "write_spec");
}

#[test]
fn sdd_yaml_flag_is_optional_but_still_supported() {
    let root = tempdir().unwrap();
    let root = root.path().to_str().unwrap();
    let output = wiki(&["sdd", "new", "explicit yaml", "--path", root, "--yaml"]);

    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .starts_with("schemaVersion: wiki.sdd/v1\n"));
}

#[test]
fn sdd_yaml_and_json_flags_conflict() {
    let root = tempdir().unwrap();
    let root = root.path().to_str().unwrap();
    let output = wiki(&[
        "sdd",
        "new",
        "format conflict",
        "--path",
        root,
        "--yaml",
        "--json",
    ]);

    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("cannot be used with"));
}

#[test]
fn sdd_list_returns_empty_yaml_array_for_an_empty_wiki() {
    let root = tempdir().unwrap();
    let root = root.path().to_str().unwrap();
    let output = wiki(&["sdd", "list", "--path", root]);

    assert!(output.status.success());
    let response: serde_yaml::Value = serde_yaml::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schemaVersion"], "wiki.sdd/v1");
    assert_eq!(response["command"], "sdd.list");
    assert_eq!(response["changes"].as_sequence().unwrap().len(), 0);
}

#[test]
fn sdd_list_defaults_to_active_changes_only() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let archived_id = create_change(&root_path, "archived list item");
    write_markdown_spec(root.path(), &archived_id);
    let verify = wiki_owned(&[
        "sdd".into(),
        "verify".into(),
        "--change".into(),
        archived_id.clone(),
        "--path".into(),
        root_path.clone(),
    ]);
    assert!(verify.status.success());
    let archive = wiki_owned(&[
        "sdd".into(),
        "archive".into(),
        "--change".into(),
        archived_id.clone(),
        "--path".into(),
        root_path.clone(),
    ]);
    assert!(archive.status.success());
    let active_id = create_change(&root_path, "active list item");

    let output = wiki_owned(&[
        "sdd".into(),
        "list".into(),
        "--path".into(),
        root_path.clone(),
        "--json".into(),
    ]);
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let changes = response["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    let active = changes
        .iter()
        .find(|change| change["id"] == active_id)
        .unwrap();
    assert_eq!(active["status"], "draft");
    assert!(active["path"].as_str().unwrap().contains("sdd/changes/"));
    assert!(changes.iter().all(|change| change["id"] != archived_id));

    let output = wiki_owned(&[
        "sdd".into(),
        "list".into(),
        "--path".into(),
        root_path,
        "--all".into(),
        "--json".into(),
    ]);
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let changes = response["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes[0]["id"].as_str().unwrap() < changes[1]["id"].as_str().unwrap());
    let archived = changes
        .iter()
        .find(|change| change["id"] == archived_id)
        .unwrap();
    assert_eq!(archived["status"], "archived");
    assert!(archived["path"].as_str().unwrap().contains("sdd/archives/"));
}

#[test]
fn sdd_lifecycle_uses_agent_written_spec_verify_and_archive() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let change_id = create_change(&root_path, "minimal lifecycle");
    write_markdown_spec(root.path(), &change_id);

    let verify = wiki_owned(&[
        "sdd".into(),
        "verify".into(),
        "--change".into(),
        change_id.clone(),
        "--path".into(),
        root_path.clone(),
    ]);
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verified_yaml: serde_yaml::Value = serde_yaml::from_slice(&verify.stdout).unwrap();
    assert_eq!(
        verified_yaml["verification"]["result"].as_str(),
        Some("pass")
    );
    let change_path = root
        .path()
        .join("sdd/changes")
        .join(&change_id)
        .join("change.md");
    let change_text = fs::read_to_string(&change_path).unwrap();
    assert_eq!(change_text.lines().filter(|line| *line == "---").count(), 2);
    assert_eq!(change_text.matches("sdd_version: 1").count(), 1);
    assert!(change_text.ends_with("# minimal lifecycle\n\n## 原始需求\n\nminimal lifecycle\n"));

    let archived = wiki_owned(&[
        "sdd".into(),
        "archive".into(),
        "--change".into(),
        change_id.clone(),
        "--path".into(),
        root_path,
    ]);
    assert!(archived.status.success());
    assert!(!root.path().join("sdd/changes").join(&change_id).exists());
    let archive_path = root
        .path()
        .join("sdd/archives")
        .join(&change_id)
        .join("change.md");
    assert!(archive_path.exists());
    let archived_text = fs::read_to_string(archive_path).unwrap();
    assert_eq!(
        archived_text.lines().filter(|line| *line == "---").count(),
        2
    );
    assert_eq!(archived_text.matches("sdd_version: 1").count(), 1);
}

#[test]
fn verify_does_not_offer_archive_when_a_task_is_unfinished() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let change_id = create_change(&root_path, "unfinished task");
    write_tasked_spec(root.path(), &change_id, true);
    fs::write(
        root.path()
            .join("sdd/changes")
            .join(&change_id)
            .join("tasks/TASK-001.md"),
        "# TASK-001：实现取消能力\n\n- Priority: must ｜ Status: doing ｜ Type: code\n- Requirement refs: REQ-001\n- Acceptance refs: AC-001\n- Evidence refs: E-001\n- Depends on: 无\n- Blocked reason: 无\n\n## 完成定义\n\n- [ ] 代码与测试均完成\n",
    )
    .unwrap();

    let verify = wiki_owned(&[
        "sdd".into(),
        "verify".into(),
        "--change".into(),
        change_id,
        "--path".into(),
        root_path,
    ]);
    assert!(verify.status.success());
    let response: serde_yaml::Value = serde_yaml::from_slice(&verify.stdout).unwrap();
    assert_eq!(response["verification"]["result"], "fail");
    assert_eq!(response["change"]["status"], "needs_fix");
    assert!(!response["next"]
        .as_sequence()
        .unwrap()
        .iter()
        .any(|item| item.as_str().unwrap().contains("archive")));
}

#[test]
fn removed_sdd_spec_and_old_stages_are_not_available_as_commands() {
    for command in ["spec", "design", "plan", "build"] {
        let output = wiki(&["sdd", command, "--help"]);
        assert!(!output.status.success(), "{command} should be removed");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("unrecognized subcommand"), "{stderr}");
    }
}

#[test]
fn agent_written_spec_preserves_full_design_record() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let change_id = create_change(&root_path, "full design record");
    write_markdown_spec(root.path(), &change_id);
    let index = fs::read_to_string(
        root.path()
            .join("sdd/changes")
            .join(&change_id)
            .join("spec/index.md"),
    )
    .unwrap();
    for heading in [
        "## 证据链",
        "## 探索记录",
        "## 决策记录",
        "## 方案比较",
        "## 影响分析矩阵",
        "## 接口契约与外部对齐",
        "## 外部接口依赖与版本依据",
        "## 非代码改动与人工对齐",
        "## 高危操作及人工复核",
        "## 风险分级与发布护栏",
        "## 测试策略",
    ] {
        assert!(index.contains(heading), "missing {heading}: {index}");
    }
}

#[test]
fn structural_verification_enters_needs_fix_when_requirement_artifact_is_missing() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let change_id = create_change(&root_path, "missing requirement artifact");
    write_markdown_spec(root.path(), &change_id);
    fs::remove_file(
        root.path()
            .join("sdd/changes")
            .join(&change_id)
            .join("spec/requirements/REQ-001.md"),
    )
    .unwrap();

    let verify = wiki_owned(&[
        "sdd".into(),
        "verify".into(),
        "--change".into(),
        change_id,
        "--path".into(),
        root_path,
    ]);
    assert!(verify.status.success());
    let result: serde_yaml::Value = serde_yaml::from_slice(&verify.stdout).unwrap();
    assert_eq!(result["verification"]["result"].as_str(), Some("fail"));
    assert_eq!(result["change"]["status"].as_str(), Some("needs_fix"));
}

#[test]
fn verification_pass_requires_every_check_to_pass() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let change_id = create_change(&root_path, "verification mismatch");
    write_markdown_spec(root.path(), &change_id);
    let verify_path = root.path().join("verify.yaml");
    fs::write(
        &verify_path,
        "schemaVersion: sdd.verification/v1\nresult: pass\nchecks:\n  - name: unit\n    status: passed\n    output: ok\n    acceptanceRefs: [AC-001]\n  - name: integration\n    status: failed\n    output: failed\n    acceptanceRefs: [AC-001]\n",
    )
    .unwrap();
    let verify = wiki_owned(&[
        "sdd".into(),
        "verify".into(),
        "--change".into(),
        change_id,
        "--path".into(),
        root_path,
        "--input".into(),
        verify_path.to_str().unwrap().to_string(),
    ]);
    assert!(verify.status.success());
    let response: serde_yaml::Value = serde_yaml::from_slice(&verify.stdout).unwrap();
    assert_eq!(response["verification"]["result"], "fail");
    assert_eq!(response["change"]["status"], "needs_fix");
}

#[test]
fn archive_rejects_unverified_agent_written_spec() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let change_id = create_change(&root_path, "archive too early");
    write_markdown_spec(root.path(), &change_id);
    let output = wiki_owned(&[
        "sdd".into(),
        "archive".into(),
        "--change".into(),
        change_id,
        "--path".into(),
        root_path,
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("SDD_ARCHIVE_NOT_READY"));
}

#[test]
fn tasked_spec_requires_bidirectional_coverage() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let change_id = create_change(&root_path, "task coverage");
    write_tasked_spec(root.path(), &change_id, false);

    let verify = wiki_owned(&[
        "sdd".into(),
        "verify".into(),
        "--change".into(),
        change_id,
        "--path".into(),
        root_path,
    ]);
    assert!(verify.status.success());
    let response: serde_yaml::Value = serde_yaml::from_slice(&verify.stdout).unwrap();
    assert_eq!(response["verification"]["result"], "fail");
    assert_eq!(response["change"]["status"], "needs_fix");
}

#[test]
fn evidence_verification_checks_task_snapshot_and_traceability() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let change_id = create_change(&root_path, "task evidence");
    write_tasked_spec(root.path(), &change_id, true);
    let verify_path = root.path().join("verify.yaml");
    fs::write(
        &verify_path,
        "schemaVersion: sdd.verification/v1\nresult: pass\ntasks:\n  - id: TASK-001\n    status: done\n    evidenceRefs: [E-001]\nchecks:\n  - id: VERIFY-001\n    name: 订单取消测试\n    status: passed\n    command: cargo test --quiet\n    output: all tests passed\n    evidenceRefs: [E-001]\n    requirementRefs: [REQ-001]\n    acceptanceRefs: [AC-001]\n    taskRefs: [TASK-001]\n",
    )
    .unwrap();

    let verify = wiki_owned(&[
        "sdd".into(),
        "verify".into(),
        "--change".into(),
        change_id,
        "--path".into(),
        root_path,
        "--input".into(),
        verify_path.to_str().unwrap().to_string(),
    ]);
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stdout)
    );
    let response: serde_yaml::Value = serde_yaml::from_slice(&verify.stdout).unwrap();
    assert_eq!(response["verification"]["result"], "pass");
    assert_eq!(response["change"]["status"], "verified");
}

#[test]
fn archive_rejects_spec_or_task_changes_after_verification() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_str().unwrap().to_string();
    let change_id = create_change(&root_path, "stale verification");
    write_markdown_spec(root.path(), &change_id);
    let verify = wiki_owned(&[
        "sdd".into(),
        "verify".into(),
        "--change".into(),
        change_id.clone(),
        "--path".into(),
        root_path.clone(),
    ]);
    assert!(verify.status.success());
    fs::write(
        root.path()
            .join("sdd/changes")
            .join(&change_id)
            .join("tasks/index.md"),
        "# 任务\n\n- Has tasks: false\n\n## Log\n\n- 2026-09-02：验证后发生变更\n",
    )
    .unwrap();

    let archive = wiki_owned(&[
        "sdd".into(),
        "archive".into(),
        "--change".into(),
        change_id,
        "--path".into(),
        root_path,
    ]);
    assert!(!archive.status.success());
    assert!(String::from_utf8(archive.stdout)
        .unwrap()
        .contains("SDD_VERIFICATION_STALE"));
}
