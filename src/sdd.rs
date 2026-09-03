//! Agent-authored Markdown Spec-Driven Development workflow.
//!
//! SDD uses Markdown as its persistent source of truth. The CLI emits YAML by
//! default (or JSON when requested) and tells the Agent where to write the
//! specification. Verification evidence may be supplied as YAML/JSON. Completed
//! changes are moved atomically to `sdd/archives/<change-id>/`.

use crate::sdd_render::{render_structural_verification, render_verification};
use crate::sdd_types::*;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use time::macros::format_description;
use time::OffsetDateTime;

const DEFAULT_ROOT: &str = "docs";
const SDD_DIR: &str = "sdd";
const CHANGES_DIR: &str = "changes";
const ARCHIVES_DIR: &str = "archives";
const PROTOCOL_VERSION: &str = "wiki.sdd/v1";
const MAX_INPUT_BYTES: usize = 1_048_576;

pub use crate::sdd_types::{SddError, SddResponse};

pub fn error_output(command: &str, error: &SddError, json: bool) -> String {
    let text = error.to_string();
    let (code, message) = match text.split_once(':') {
        Some((code, message)) if code.starts_with("SDD_") => (code.trim(), message.trim()),
        _ => ("SDD_ERROR", text.as_str()),
    };
    let response = SddResponse::error(command, code, message);
    if json {
        serde_json::to_string_pretty(&response).unwrap_or_else(|_| format!("{{\"schemaVersion\":\"{PROTOCOL_VERSION}\",\"ok\":false,\"command\":\"{command}\",\"error\":{{\"code\":\"{code}\",\"message\":\"{message}\"}}}}"))
    } else {
        serde_yaml::to_string(&response).unwrap_or_else(|_| {
            format!("schemaVersion: {PROTOCOL_VERSION}\nok: false\ncommand: {command}\nerror:\n  code: {code}\n  message: {message}\n")
        })
    }
}

pub fn new(root: Option<&str>, title: &str) -> Result<SddResponse, SddError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(SddError::Invalid(
            "SDD_INVALID_TITLE: title must not be empty.".into(),
        ));
    }
    let root = resolve_root(root, true)?;
    let base = root.join(SDD_DIR).join(CHANGES_DIR);
    fs::create_dir_all(&base).map_err(io_error)?;
    let id = make_change_id(title, &base);
    let change_dir = base.join(&id);
    fs::create_dir_all(&change_dir).map_err(io_error)?;
    let now = timestamp();
    let meta = ChangeMeta {
        sdd_version: 1,
        change_id: id.clone(),
        title: title.to_string(),
        status: "draft".into(),
        phase: "spec".into(),
        created_at: now.clone(),
        updated_at: now,
        verified_fingerprint: None,
    };
    write_change(&change_dir, &meta, Some(title))?;
    let mut response = response_for(&root, &change_dir, &meta, "sdd.new");
    response.action_required = Some(ActionRequired {
        kind: "write_spec".into(),
        path: relative(&root, &change_dir),
        instructions: vec![
            "阅读知识库根目录的 SDD.md，按 evidence-first 规则先探索再写规格。".into(),
            "直接创建 spec/index.md，写清问题、目标、非目标、约束、风险、探索、决策、影响和测试策略。".into(),
            "为每个需求创建 spec/requirements/REQ-*.md，并使用稳定的 REQ-* 与 AC-*/ACC-* 编号。".into(),
            "按需求拆分 tasks/index.md 与 tasks/TASK-*.md，建立无环依赖和 REQ/AC 覆盖矩阵；微小变更可声明 Has tasks: false。".into(),
            "每个 must 需求至少包含一个 Given/When/Then 验收场景，并在结论处标注 E-* 证据。".into(),
            format!("规格完成后运行 wiki sdd verify --change {id}"),
        ],
    });
    response.next.push(format!(
        "Agent 直接编写 sdd/changes/{id}/spec/ 与 tasks/ 下的 Markdown 后运行 wiki sdd verify --change {id}"
    ));
    Ok(response)
}

/// List active SDD changes, optionally including archived changes, from the
/// Markdown source of truth.
pub fn list(root: Option<&str>, include_archived: bool) -> Result<SddResponse, SddError> {
    let root = resolve_root(root, false)?;
    let mut changes = Vec::new();
    let directories = if include_archived {
        vec![CHANGES_DIR, ARCHIVES_DIR]
    } else {
        vec![CHANGES_DIR]
    };
    for directory in directories {
        let base = root.join(SDD_DIR).join(directory);
        if !base.exists() {
            continue;
        }
        if !base.is_dir() {
            return Err(SddError::Invalid(format!(
                "SDD_PATH_NOT_DIRECTORY: {}",
                base.display()
            )));
        }
        let mut entries = fs::read_dir(&base)
            .map_err(io_error)?
            .map(|entry| entry.map(|entry| entry.path()).map_err(io_error))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort();
        for dir in entries.into_iter().filter(|path| path.is_dir()) {
            let change_id = dir
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    SddError::Invalid(format!("SDD_INVALID_CHANGE_ID: {}", dir.display()))
                })?;
            validate_change_id(change_id)?;
            let (_, meta) = load_change_from_dir(&dir, change_id)?;
            changes.push(summary(&root, &dir, &meta));
        }
    }
    changes.sort_by(|left, right| left.id.cmp(&right.id).then(left.path.cmp(&right.path)));
    let mut response = SddResponse::success("sdd.list");
    response.changes = Some(changes);
    Ok(response)
}

pub fn verify(
    root: Option<&str>,
    change_id: &str,
    input: Option<&str>,
) -> Result<SddResponse, SddError> {
    let root = resolve_root(root, false)?;
    let (change_dir, mut meta) = load_change(&root, change_id)?;
    if !matches!(meta.status.as_str(), "draft" | "needs_fix" | "verified") {
        return Err(SddError::Invalid(format!(
            "SDD_VERIFY_NOT_READY: status is {}, expected draft or needs_fix.",
            meta.status
        )));
    }
    let structure = inspect_spec_structure(&change_dir)?;
    let requirement_count = structure.requirements.len();
    let mut structural_issues = structure.issues.clone();
    for task in unfinished_tasks(&structure) {
        structural_issues.push(format!("任务 {} 尚未完成：{}", task.id, task.status));
    }
    let mut issues = structural_issues.clone();
    let structural_failed = !structural_issues.is_empty();
    let (mut result, checks, mut passed, mut failed) = if let Some(input) = input {
        let verification: VerificationInput = parse_yaml(input, "sdd.verification/v1")?;
        validate_verification(&verification)?;
        issues.extend(inspect_verification(&verification, &structure));
        let passed = verification
            .checks
            .iter()
            .filter(|check| {
                check.status.eq_ignore_ascii_case("passed")
                    || check.status.eq_ignore_ascii_case("pass")
            })
            .count();
        let requested_pass = matches!(
            verification.result.to_ascii_lowercase().as_str(),
            "pass" | "passed"
        );
        let result = if requested_pass && issues.is_empty() {
            "pass".to_string()
        } else {
            "fail".to_string()
        };
        let failed = verification
            .checks
            .len()
            .saturating_sub(passed)
            .saturating_add(usize::from(!issues.is_empty()));
        render_verification(&change_dir, &verification, structural_failed, &issues)?;
        (result, verification.checks.len(), passed, failed)
    } else {
        let result = if structural_failed {
            "fail".to_string()
        } else {
            "pass".to_string()
        };
        render_structural_verification(
            &change_dir,
            &result,
            requirement_count,
            structure.tasks.len(),
            structure.has_tasks,
            structural_failed,
            &structural_issues,
        )?;
        (
            result,
            requirement_count,
            requirement_count.saturating_sub(if structural_failed { 1 } else { 0 }),
            if structural_failed { 1 } else { 0 },
        )
    };
    if structural_failed
        && (result.eq_ignore_ascii_case("pass") || result.eq_ignore_ascii_case("passed"))
    {
        result = "fail".into();
        passed = passed.saturating_sub(1);
        failed = failed.saturating_add(1);
    }
    meta.status = if result.eq_ignore_ascii_case("pass") || result.eq_ignore_ascii_case("passed") {
        "verified"
    } else {
        "needs_fix"
    }
    .into();
    meta.phase = if meta.status == "verified" {
        "archive"
    } else {
        "spec"
    }
    .into();
    meta.verified_fingerprint = if meta.status == "verified" {
        Some(change_content_fingerprint(&change_dir)?)
    } else {
        None
    };
    touch_meta(&change_dir, &mut meta)?;
    let mut response = response_for(&root, &change_dir, &meta, "sdd.verify");
    response.verification = Some(VerificationSummary {
        result,
        checks,
        passed,
        failed,
    });
    if meta.status == "needs_fix" {
        response.next.push(format!(
            "验证未通过，请修复规格或补充证据后重试 wiki sdd verify --change {change_id}"
        ));
    } else {
        response.next.push(format!(
            "验证已通过，可运行 wiki sdd archive --change {change_id}"
        ));
    }
    Ok(response)
}

pub fn archive(root: Option<&str>, change_id: &str) -> Result<SddResponse, SddError> {
    let root = resolve_root(root, false)?;
    validate_change_id(change_id)?;
    let changes = root.join(SDD_DIR).join(CHANGES_DIR);
    let archives = root.join(SDD_DIR).join(ARCHIVES_DIR);
    fs::create_dir_all(&archives).map_err(io_error)?;
    let source = changes.join(change_id);
    let destination = archives.join(change_id);
    if destination.is_dir() && !source.exists() {
        let (dir, mut meta) = load_change_from_dir(&destination, change_id)?;
        meta.status = "archived".into();
        meta.phase = "archived".into();
        touch_meta(&dir, &mut meta)?;
        return Ok(archive_response(&root, &dir, &meta, &source, &destination));
    }
    let (meta_dir, meta) = load_change(&root, change_id)?;
    if meta.status != "verified" {
        return Err(SddError::Invalid(format!(
            "SDD_ARCHIVE_NOT_READY: status is {}, expected verified.",
            meta.status
        )));
    }
    let structure = inspect_spec_structure(&meta_dir)?;
    if let Some(task) = unfinished_tasks(&structure).first() {
        return Err(SddError::Invalid(format!(
            "SDD_ARCHIVE_TASKS_INCOMPLETE: 任务 {} 尚未完成：{}",
            task.id, task.status
        )));
    }
    let verified_fingerprint = meta.verified_fingerprint.as_deref().ok_or_else(|| {
        SddError::Invalid(
            "SDD_VERIFICATION_STALE: verified fingerprint is missing; rerun wiki sdd verify."
                .into(),
        )
    })?;
    let current_fingerprint = change_content_fingerprint(&meta_dir)?;
    if current_fingerprint != verified_fingerprint {
        return Err(SddError::Invalid(
            "SDD_VERIFICATION_STALE: spec/ or tasks/ changed after verification; rerun wiki sdd verify."
                .into(),
        ));
    }
    if destination.exists() {
        return Err(SddError::Invalid(format!(
            "SDD_ARCHIVE_EXISTS: {}",
            destination.display()
        )));
    }
    rename_change_directory(&meta_dir, &destination)?;
    let (dir, mut moved_meta) = load_change_from_dir(&destination, change_id)?;
    moved_meta.status = "archived".into();
    moved_meta.phase = "archived".into();
    touch_meta(&dir, &mut moved_meta)?;
    Ok(archive_response(
        &root,
        &dir,
        &moved_meta,
        &source,
        &destination,
    ))
}

fn archive_response(
    root: &Path,
    dir: &Path,
    meta: &ChangeMeta,
    source: &Path,
    destination: &Path,
) -> SddResponse {
    let mut response = response_for(root, dir, meta, "sdd.archive");
    response.moved = Some(MoveSummary {
        from: relative(root, source),
        to: relative(root, destination),
    });
    response
}

fn resolve_root(input: Option<&str>, create: bool) -> Result<PathBuf, SddError> {
    let raw = input.unwrap_or(DEFAULT_ROOT);
    if raw.trim().is_empty() {
        return Err(SddError::Invalid(
            "SDD_INVALID_PATH: path must not be empty.".into(),
        ));
    }
    let path = PathBuf::from(raw);
    if create {
        fs::create_dir_all(&path).map_err(io_error)?;
    }
    if !path.exists() {
        return Err(SddError::Invalid(format!(
            "SDD_PATH_NOT_FOUND: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(SddError::Invalid(format!(
            "SDD_PATH_NOT_DIRECTORY: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn load_change(root: &Path, change_id: &str) -> Result<(PathBuf, ChangeMeta), SddError> {
    validate_change_id(change_id)?;
    let changes = root.join(SDD_DIR).join(CHANGES_DIR).join(change_id);
    if changes.is_dir() {
        return load_change_from_dir(&changes, change_id);
    }
    let archives = root.join(SDD_DIR).join(ARCHIVES_DIR).join(change_id);
    if archives.is_dir() {
        return Err(SddError::Invalid(format!(
            "SDD_CHANGE_ARCHIVED: {change_id}"
        )));
    }
    Err(SddError::Invalid(format!(
        "SDD_CHANGE_NOT_FOUND: {change_id}"
    )))
}

fn load_change_from_dir(dir: &Path, change_id: &str) -> Result<(PathBuf, ChangeMeta), SddError> {
    let meta: ChangeMeta = load_frontmatter(&dir.join("change.md"))?;
    if meta.change_id != change_id {
        return Err(SddError::Invalid("SDD_CHANGE_ID_MISMATCH".into()));
    }
    Ok((dir.to_path_buf(), meta))
}

fn response_for(root: &Path, dir: &Path, meta: &ChangeMeta, command: &str) -> SddResponse {
    let mut response = SddResponse::success(command);
    response.change = Some(summary(root, dir, meta));
    response
        .artifacts
        .insert("change".into(), relative(root, &dir.join("change.md")));
    if dir.join("spec").is_dir() {
        response
            .artifacts
            .insert("spec".into(), relative(root, &dir.join("spec")));
    }
    for name in ["verify.md"] {
        let path = dir.join(name);
        if path.is_file() {
            response
                .artifacts
                .insert(name.trim_end_matches(".md").into(), relative(root, &path));
        }
    }
    response
}

fn summary(root: &Path, dir: &Path, meta: &ChangeMeta) -> ChangeSummary {
    ChangeSummary {
        id: meta.change_id.clone(),
        title: meta.title.clone(),
        status: meta.status.clone(),
        phase: meta.phase.clone(),
        path: relative(root, dir),
    }
}

fn validate_change_id(value: &str) -> Result<(), SddError> {
    if value.is_empty() || value.len() > 80 || value == "." || value == ".." {
        return Err(SddError::Invalid("SDD_INVALID_CHANGE_ID".into()));
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
    }) {
        return Err(SddError::Invalid(
            "SDD_INVALID_CHANGE_ID: only lowercase letters, digits, '-', '_' and '.' are allowed."
                .into(),
        ));
    }
    Ok(())
}

fn make_change_id(title: &str, base: &Path) -> String {
    let slug: String = title
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "change" } else { &slug };
    let date = OffsetDateTime::now_utc().date();
    let date = date.to_string().replace('-', "");
    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let prefix = format!("{date}-{slug}-{}", &digest[..8]);
    if !change_id_exists(base, &prefix) {
        return prefix;
    }
    for index in 2..10_000 {
        let candidate = format!("{prefix}-{index}");
        if !change_id_exists(base, &candidate) {
            return candidate;
        }
    }
    format!("{prefix}-{}", timestamp().replace([':', '-', 'T', 'Z'], ""))
}

fn change_id_exists(changes_base: &Path, change_id: &str) -> bool {
    changes_base.join(change_id).exists()
        || changes_base
            .parent()
            .map(|sdd| sdd.join(ARCHIVES_DIR).join(change_id).exists())
            .unwrap_or(false)
}

fn timestamp() -> String {
    let format = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    OffsetDateTime::now_utc()
        .format(&format)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

fn change_content_fingerprint(dir: &Path) -> Result<String, SddError> {
    let mut files = Vec::new();
    for name in ["spec", "tasks"] {
        collect_files(&dir.join(name), dir, &mut files)?;
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, path) in files {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(fs::read(path).map_err(io_error)?);
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_files(
    path: &Path,
    base: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), SddError> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        files.push((relative(base, path), path.to_path_buf()));
        return Ok(());
    }
    let mut entries = fs::read_dir(path)
        .map_err(io_error)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(io_error))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    for entry in entries {
        collect_files(&entry, base, files)?;
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn io_error(error: std::io::Error) -> SddError {
    SddError::Io(error.to_string())
}

fn rename_change_directory(source: &Path, destination: &Path) -> Result<(), SddError> {
    rename_with_retry(
        source,
        destination,
        "SDD_ARCHIVE_MOVE_FAILED",
        "Windows 可能仍有编辑器、终端、索引器或杀毒软件占用 change 目录；请关闭占用后重试。",
    )
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), SddError> {
    // Windows does not allow rename-overwrite when the destination file exists.
    // Copying over the existing file preserves the destination path semantics.
    if cfg!(windows) && destination.is_file() {
        let max_attempts = 8;
        let mut delay = Duration::from_millis(25);
        let mut last_error = None;
        for attempt in 0..max_attempts {
            match fs::copy(source, destination) {
                Ok(_) => {
                    fs::remove_file(source).map_err(|error| {
                        SddError::Invalid(format!(
                            "SDD_FILE_REPLACE_FAILED: cannot remove temporary file {}: {}",
                            source.display(),
                            error
                        ))
                    })?;
                    return Ok(());
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::PermissionDenied
                        && attempt + 1 < max_attempts =>
                {
                    last_error = Some(error);
                    thread::sleep(delay);
                    delay = std::cmp::min(delay.saturating_mul(2), Duration::from_millis(500));
                }
                Err(error) => {
                    return Err(rename_error(
                        source,
                        destination,
                        error,
                        "SDD_FILE_REPLACE_FAILED",
                        "Windows 可能仍有编辑器、索引器或杀毒软件占用目标文件；请关闭占用后重试。",
                    ));
                }
            }
        }
        return Err(rename_error(
            source,
            destination,
            last_error.expect("copy retry loop must retain the last error"),
            "SDD_FILE_REPLACE_FAILED",
            "Windows 可能仍有编辑器、索引器或杀毒软件占用目标文件；请关闭占用后重试。",
        ));
    }
    rename_with_retry(
        source,
        destination,
        "SDD_FILE_REPLACE_FAILED",
        "Windows 可能仍有编辑器、索引器或杀毒软件占用目标文件；请关闭占用后重试。",
    )
}

fn rename_with_retry(
    source: &Path,
    destination: &Path,
    error_code: &str,
    windows_hint: &str,
) -> Result<(), SddError> {
    let max_attempts = if cfg!(windows) { 8 } else { 1 };
    let mut delay = Duration::from_millis(25);
    let mut last_error = None;

    for attempt in 0..max_attempts {
        match fs::rename(source, destination) {
            Ok(()) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && error.kind() == std::io::ErrorKind::PermissionDenied
                    && attempt + 1 < max_attempts =>
            {
                last_error = Some(error);
                thread::sleep(delay);
                delay = std::cmp::min(delay.saturating_mul(2), Duration::from_millis(500));
            }
            Err(error) => {
                return Err(rename_error(
                    source,
                    destination,
                    error,
                    error_code,
                    windows_hint,
                ));
            }
        }
    }

    Err(rename_error(
        source,
        destination,
        last_error.expect("rename retry loop must retain the last error"),
        error_code,
        windows_hint,
    ))
}

fn rename_error(
    source: &Path,
    destination: &Path,
    error: std::io::Error,
    error_code: &str,
    windows_hint: &str,
) -> SddError {
    if cfg!(windows) && error.kind() == std::io::ErrorKind::PermissionDenied {
        return SddError::Invalid(format!(
            "{error_code}: {} -> {}: {}. {windows_hint}",
            source.display(),
            destination.display(),
            error,
        ));
    }
    SddError::Io(format!(
        "{error_code}: {} -> {}: {}",
        source.display(),
        destination.display(),
        error
    ))
}

fn read_input(value: &str) -> Result<String, SddError> {
    if value == "-" {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(io_error)?;
        if input.len() > MAX_INPUT_BYTES {
            return Err(SddError::Invalid("SDD_INPUT_TOO_LARGE".into()));
        }
        return Ok(input);
    }
    let bytes = fs::read(value).map_err(io_error)?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(SddError::Invalid("SDD_INPUT_TOO_LARGE".into()));
    }
    String::from_utf8(bytes).map_err(|_| SddError::Invalid("SDD_INPUT_NOT_UTF8".into()))
}

pub fn load_input(value: Option<&str>) -> Result<Option<String>, SddError> {
    value.map(read_input).transpose()
}

fn parse_yaml<T: for<'de> Deserialize<'de>>(
    input: &str,
    expected_schema: &str,
) -> Result<T, SddError> {
    let value: Value = serde_yaml::from_str(input)
        .map_err(|error| SddError::Yaml(format!("SDD_INPUT_INVALID_YAML: {error}")))?;
    let schema = value
        .get("schemaVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if schema != expected_schema {
        return Err(SddError::Yaml(format!(
            "SDD_SCHEMA_MISMATCH: expected {expected_schema}, got {schema}"
        )));
    }
    serde_yaml::from_value(value)
        .map_err(|error| SddError::Yaml(format!("SDD_INPUT_INVALID_YAML: {error}")))
}

fn load_frontmatter<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, SddError> {
    let text = fs::read_to_string(path).map_err(io_error)?;
    let front = extract_frontmatter(&text)?;
    serde_yaml::from_str(front)
        .map_err(|error| SddError::Yaml(format!("SDD_INVALID_MARKDOWN_FRONTMATTER: {error}")))
}

fn extract_frontmatter(text: &str) -> Result<&str, SddError> {
    split_frontmatter(text).map(|(front, _)| front)
}

fn split_frontmatter(text: &str) -> Result<(&str, &str), SddError> {
    let content = if let Some(content) = text.strip_prefix("---\n") {
        content
    } else {
        return Err(SddError::Invalid(
            "SDD_INVALID_MARKDOWN: missing front matter.".into(),
        ));
    };
    let end = content.find("\n---").ok_or_else(|| {
        SddError::Invalid("SDD_INVALID_MARKDOWN: unterminated front matter.".into())
    })?;
    let body_start = end + "\n---".len();
    let body = content[body_start..]
        .strip_prefix('\n')
        .unwrap_or(&content[body_start..]);
    Ok((&content[..end], body))
}

fn write_change(dir: &Path, meta: &ChangeMeta, request: Option<&str>) -> Result<(), SddError> {
    let body = format!(
        "# {}\n\n## 原始需求\n\n{}\n",
        meta.title,
        request.unwrap_or_default()
    );
    write_markdown(&dir.join("change.md"), meta, &body)
}

fn touch_meta(dir: &Path, meta: &mut ChangeMeta) -> Result<(), SddError> {
    meta.updated_at = timestamp();
    let path = dir.join("change.md");
    let text = fs::read_to_string(&path).map_err(io_error)?;
    let (_, body) = split_frontmatter(&text)?;
    write_markdown(&path, meta, body)
}

pub(crate) fn write_markdown<T: Serialize>(
    path: &Path,
    front: &T,
    body: &str,
) -> Result<(), SddError> {
    let yaml = serde_yaml::to_string(front).map_err(|error| SddError::Yaml(error.to_string()))?;
    let content = format!("---\n{}---\n\n{}", yaml, body.trim_start());
    let temp = path.with_extension("tmp");
    fs::write(&temp, content).map_err(io_error)?;
    replace_file(&temp, path)
}

#[derive(Debug, Default)]
struct RequirementDoc {
    id: String,
    priority: String,
    status: String,
    acceptance_ids: Vec<String>,
    dependencies: Vec<String>,
}

#[derive(Debug, Default)]
struct TaskDoc {
    id: String,
    priority: String,
    status: String,
    task_type: String,
    requirement_refs: Vec<String>,
    acceptance_refs: Vec<String>,
    dependencies: Vec<String>,
    blocked_reason: Option<String>,
    unblock_condition: Option<String>,
    dropped_reason: Option<String>,
    has_definition: bool,
}

#[derive(Debug, Default)]
struct CoverageRow {
    requirement: String,
    acceptance: String,
    task: String,
}

#[derive(Debug, Default)]
struct ChangeStructure {
    requirements: Vec<RequirementDoc>,
    tasks: Vec<TaskDoc>,
    coverage: Vec<CoverageRow>,
    has_tasks: bool,
    evidence_ids: BTreeSet<String>,
    issues: Vec<String>,
}

fn inspect_spec_structure(dir: &Path) -> Result<ChangeStructure, SddError> {
    let mut structure = ChangeStructure::default();
    let spec_dir = dir.join("spec");
    let index = spec_dir.join("index.md");
    let requirements_dir = spec_dir.join("requirements");
    let mut texts = Vec::new();
    if !index.is_file() {
        structure.issues.push("spec/index.md 缺失".into());
    } else {
        let text = fs::read_to_string(&index).map_err(io_error)?;
        if text.trim().is_empty() {
            structure.issues.push("spec/index.md 为空".into());
        }
        texts.push((index, text));
    }
    if !requirements_dir.is_dir() {
        structure.issues.push("spec/requirements 目录缺失".into());
    } else {
        let mut paths = markdown_files(&requirements_dir)?;
        paths.sort();
        for path in paths {
            let text = fs::read_to_string(&path).map_err(io_error)?;
            if text.trim().is_empty() {
                structure.issues.push(format!("{} 为空", path.display()));
            }
            let requirement = parse_requirement(&path, &text);
            if requirement.id.is_empty() {
                structure
                    .issues
                    .push(format!("{} 缺少 REQ-* 标识", path.display()));
            }
            if !text.contains("## 验收条件") {
                structure
                    .issues
                    .push(format!("{} 缺少验收条件", path.display()));
            }
            structure.requirements.push(requirement);
            texts.push((path, text));
        }
    }
    if structure.requirements.is_empty() {
        structure
            .issues
            .push("至少需要一个 requirements/*.md".into());
    }

    let tasks_dir = dir.join("tasks");
    let tasks_index = tasks_dir.join("index.md");
    if !tasks_index.is_file() {
        structure.issues.push("tasks/index.md 缺失".into());
        structure.has_tasks = true;
    } else {
        let text = fs::read_to_string(&tasks_index).map_err(io_error)?;
        structure.has_tasks = !has_tasks_false(&text);
        structure.coverage = parse_coverage(&text);
        texts.push((tasks_index, text));
        if !structure.has_tasks && tasks_dir.is_dir() {
            let has_task_files = markdown_files(&tasks_dir)?.into_iter().any(|path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(|stem| stem.starts_with("TASK-"))
                    .unwrap_or(false)
            });
            if has_task_files {
                structure
                    .issues
                    .push("tasks/index.md 声明 Has tasks: false，但仍存在 TASK-*.md".into());
            }
        }
    }
    if structure.has_tasks {
        if !tasks_dir.is_dir() {
            structure.issues.push("tasks/ 目录缺失".into());
        } else {
            let mut paths = markdown_files(&tasks_dir)?;
            paths
                .retain(|path| path.file_name().and_then(|name| name.to_str()) != Some("index.md"));
            paths.sort();
            for path in paths {
                let text = fs::read_to_string(&path).map_err(io_error)?;
                let task = parse_task(&path, &text);
                if task.id.is_empty() {
                    continue;
                }
                if text.trim().is_empty() {
                    structure.issues.push(format!("{} 为空", path.display()));
                }
                structure.tasks.push(task);
                texts.push((path, text));
            }
            if structure.tasks.is_empty() {
                structure.issues.push(
                    "至少需要一个 tasks/TASK-*.md，或在 tasks/index.md 声明 Has tasks: false"
                        .into(),
                );
            }
        }
    }

    validate_structure_ids(&mut structure, &texts);
    validate_requirement_dependencies(&mut structure);
    validate_tasks(&mut structure);
    Ok(structure)
}

fn markdown_files(dir: &Path) -> Result<Vec<PathBuf>, SddError> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn parse_requirement(path: &Path, text: &str) -> RequirementDoc {
    let id = first_id(text, "REQ-")
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let priority = field_value(text, "priority").unwrap_or_else(|| "must".into());
    let status = field_value(text, "status").unwrap_or_else(|| "proposed".into());
    let acceptance_ids = heading_ids(text, &["AC-", "ACC-"]);
    let dependencies = field_ids(text, "dependencies", "REQ-");
    RequirementDoc {
        id,
        priority,
        status,
        acceptance_ids,
        dependencies,
    }
}

fn parse_task(_path: &Path, text: &str) -> TaskDoc {
    let id = first_id(text, "TASK-").unwrap_or_default();
    let priority = field_value(text, "priority").unwrap_or_else(|| "must".into());
    let status = field_value(text, "status").unwrap_or_else(|| "todo".into());
    let task_type = field_value(text, "type").unwrap_or_else(|| "code".into());
    let requirement_refs = field_ids(text, "requirement refs", "REQ-");
    let acceptance_refs = field_ids_any(text, "acceptance refs", &["AC-", "ACC-"]);
    let dependencies = field_ids(text, "depends on", "TASK-");
    let blocked_reason = field_value(text, "blocked reason");
    let unblock_condition = field_value(text, "unblock condition");
    let dropped_reason = field_value(text, "dropped reason");
    let has_definition = text.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("## completion definition") || line.contains("## 完成定义")
    });
    TaskDoc {
        id,
        priority,
        status,
        task_type,
        requirement_refs,
        acceptance_refs,
        dependencies,
        blocked_reason,
        unblock_condition,
        dropped_reason,
        has_definition,
    }
}

fn has_tasks_false(text: &str) -> bool {
    field_value(text, "has tasks")
        .map(|value| value.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}

fn field_value(text: &str, field: &str) -> Option<String> {
    let field = field.to_ascii_lowercase();
    for line in text.lines() {
        for segment in line.split(['｜', '|']) {
            let segment = segment.trim().trim_start_matches('-').trim();
            let pair = segment.split_once(':').or_else(|| segment.split_once('：'));
            let Some((key, value)) = pair else { continue };
            if key.trim().to_ascii_lowercase() == field {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

fn field_ids(text: &str, field: &str, prefix: &str) -> Vec<String> {
    field_value(text, field)
        .map(|value| extract_ids(&value, prefix))
        .unwrap_or_default()
}

fn field_ids_any(text: &str, field: &str, prefixes: &[&str]) -> Vec<String> {
    let value = field_value(text, field).unwrap_or_default();
    let mut ids = Vec::new();
    for prefix in prefixes {
        ids.extend(extract_ids(&value, prefix));
    }
    ids.sort();
    ids.dedup();
    ids
}

fn first_id(text: &str, prefix: &str) -> Option<String> {
    text.lines()
        .filter(|line| line.trim_start().starts_with('#'))
        .find_map(|line| extract_ids(line, prefix).into_iter().next())
}

fn heading_ids(text: &str, prefixes: &[&str]) -> Vec<String> {
    let mut ids = Vec::new();
    for line in text
        .lines()
        .filter(|line| line.trim_start().starts_with('#'))
    {
        for prefix in prefixes {
            ids.extend(extract_ids(line, prefix));
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn extract_ids(text: &str, prefix: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for token in text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_') {
        if token.starts_with(prefix) && token.len() > prefix.len() {
            let suffix = &token[prefix.len()..];
            if suffix
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
            {
                ids.push(token.to_string());
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn parse_coverage(text: &str) -> Vec<CoverageRow> {
    let mut rows = Vec::new();
    for line in text
        .lines()
        .filter(|line| line.trim_start().starts_with('|'))
    {
        let cells: Vec<_> = line.split('|').map(str::trim).collect();
        if cells.len() < 4
            || !cells[1].starts_with("REQ-")
            || !(cells[2].starts_with("AC-") || cells[2].starts_with("ACC-"))
            || !cells[3].starts_with("TASK-")
        {
            continue;
        }
        rows.push(CoverageRow {
            requirement: cells[1].into(),
            acceptance: cells[2].into(),
            task: cells[3].into(),
        });
    }
    rows
}

fn validate_structure_ids(structure: &mut ChangeStructure, texts: &[(PathBuf, String)]) {
    let mut req_ids = BTreeSet::new();
    let mut acceptance_ids = BTreeSet::new();
    for requirement in &structure.requirements {
        if !matches!(
            requirement.priority.to_ascii_lowercase().as_str(),
            "must" | "should" | "may"
        ) {
            structure.issues.push(format!(
                "{} 的 Priority 必须是 must/should/may",
                requirement.id
            ));
        }
        if !matches!(
            requirement.status.to_ascii_lowercase().as_str(),
            "proposed" | "implemented" | "verified" | "dropped"
        ) {
            structure.issues.push(format!(
                "{} 的 Status 必须是 proposed/implemented/verified/dropped",
                requirement.id
            ));
        }
        if !req_ids.insert(requirement.id.clone()) && !requirement.id.is_empty() {
            structure
                .issues
                .push(format!("SDD_DUPLICATE_ID: {}", requirement.id));
        }
        for id in &requirement.acceptance_ids {
            if !acceptance_ids.insert(id.clone()) {
                structure.issues.push(format!("SDD_DUPLICATE_ID: {}", id));
            }
        }
        if requirement.priority.eq_ignore_ascii_case("must")
            && requirement.acceptance_ids.is_empty()
        {
            structure
                .issues
                .push(format!("SDD_REQUIREMENT_UNCOVERED: {}", requirement.id));
        }
    }
    let mut task_ids = BTreeSet::new();
    for task in &structure.tasks {
        if !task_ids.insert(task.id.clone()) && !task.id.is_empty() {
            structure
                .issues
                .push(format!("SDD_DUPLICATE_ID: {}", task.id));
        }
    }
    for (_, text) in texts {
        for line in text.lines() {
            if ["[CODE]", "[SDD]", "[CMD]"]
                .iter()
                .any(|kind| line.contains(kind))
            {
                for id in extract_ids(line, "E-") {
                    if !structure.evidence_ids.insert(id.clone()) {
                        structure.issues.push(format!("SDD_DUPLICATE_ID: {}", id));
                    }
                }
            }
        }
    }
}

fn validate_requirement_dependencies(structure: &mut ChangeStructure) {
    let ids: BTreeSet<String> = structure
        .requirements
        .iter()
        .map(|req| req.id.clone())
        .collect();
    let mut graph = BTreeMap::new();
    for req in &structure.requirements {
        for dependency in &req.dependencies {
            if dependency == &req.id {
                structure
                    .issues
                    .push(format!("SDD_REQUIREMENT_SELF_DEPENDENCY: {}", req.id));
            }
            if !ids.contains(dependency) {
                structure.issues.push(format!(
                    "SDD_UNKNOWN_REQUIREMENT_REFERENCE: {} -> {}",
                    req.id, dependency
                ));
            }
        }
        graph.insert(req.id.clone(), req.dependencies.clone());
    }
    if has_dependency_cycle(&graph) {
        structure
            .issues
            .push("SDD_REQUIREMENT_DEPENDENCY_CYCLE".into());
    }
}

fn validate_tasks(structure: &mut ChangeStructure) {
    if !structure.has_tasks {
        return;
    }
    let req_map: BTreeMap<_, _> = structure
        .requirements
        .iter()
        .map(|req| (req.id.clone(), req))
        .collect();
    let mut task_map = BTreeMap::new();
    let mut covered = BTreeSet::new();
    for row in &structure.coverage {
        covered.insert((row.requirement.clone(), row.acceptance.clone()));
    }
    for task in &structure.tasks {
        task_map.insert(task.id.clone(), task.dependencies.clone());
        if task.requirement_refs.is_empty() {
            structure
                .issues
                .push(format!("{} 缺少 Requirement refs", task.id));
        }
        if !task.has_definition {
            structure.issues.push(format!("{} 缺少完成定义", task.id));
        }
        let status = task.status.to_ascii_lowercase();
        if !matches!(
            status.as_str(),
            "todo" | "doing" | "blocked" | "done" | "dropped"
        ) {
            structure.issues.push(format!("{} 的 Status 非法", task.id));
        }
        if !matches!(
            task.priority.to_ascii_lowercase().as_str(),
            "must" | "should" | "may"
        ) {
            structure
                .issues
                .push(format!("{} 的 Priority 必须是 must/should/may", task.id));
        }
        if !matches!(
            task.task_type.to_ascii_lowercase().as_str(),
            "code" | "test" | "config" | "data" | "docs" | "review" | "release"
        ) {
            structure.issues.push(format!("{} 的 Type 非法", task.id));
        }
        if status == "blocked"
            && task
                .blocked_reason
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        {
            structure
                .issues
                .push(format!("{} blocked 必须填写 Blocked reason", task.id));
        }
        if status == "blocked"
            && task
                .unblock_condition
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        {
            structure
                .issues
                .push(format!("{} blocked 必须填写 Unblock condition", task.id));
        }
        if status == "dropped"
            && task
                .dropped_reason
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        {
            structure
                .issues
                .push(format!("{} dropped 必须填写 Dropped reason", task.id));
        }
        for req_id in &task.requirement_refs {
            match req_map.get(req_id) {
                Some(req)
                    if req.priority.eq_ignore_ascii_case("must")
                        && !task.priority.eq_ignore_ascii_case("must") =>
                {
                    structure
                        .issues
                        .push(format!("{} 覆盖 must 需求但任务优先级不是 must", task.id))
                }
                Some(_) => {}
                None => structure
                    .issues
                    .push(format!("{} 引用了不存在的 {}", task.id, req_id)),
            }
        }
        for acceptance_id in &task.acceptance_refs {
            if !structure
                .requirements
                .iter()
                .any(|req| req.acceptance_ids.contains(acceptance_id))
            {
                structure
                    .issues
                    .push(format!("{} 引用了不存在的 {}", task.id, acceptance_id));
            }
        }
        for dependency in &task.dependencies {
            if dependency == &task.id {
                structure
                    .issues
                    .push(format!("SDD_TASK_SELF_DEPENDENCY: {}", task.id));
            }
            if !structure.tasks.iter().any(|other| &other.id == dependency) {
                structure.issues.push(format!(
                    "SDD_UNKNOWN_TASK_REFERENCE: {} -> {}",
                    task.id, dependency
                ));
            }
        }
    }
    if has_dependency_cycle(&task_map) {
        structure.issues.push("SDD_TASK_DEPENDENCY_CYCLE".into());
    }
    for req in &structure.requirements {
        if !req.priority.eq_ignore_ascii_case("must") {
            continue;
        }
        for acceptance in &req.acceptance_ids {
            let task_cover = structure.tasks.iter().any(|task| {
                !task.status.eq_ignore_ascii_case("dropped")
                    && task.acceptance_refs.contains(acceptance)
            });
            if !task_cover {
                structure
                    .issues
                    .push(format!("{} / {} 没有任务覆盖", req.id, acceptance));
            }
            if !covered.contains(&(req.id.clone(), acceptance.clone())) {
                structure
                    .issues
                    .push(format!("覆盖矩阵缺少 {} / {}", req.id, acceptance));
            }
        }
    }
    for row in &structure.coverage {
        if !task_map.contains_key(&row.task) {
            structure
                .issues
                .push(format!("覆盖矩阵引用不存在的 {}", row.task));
        }
        if !structure
            .requirements
            .iter()
            .any(|req| req.id == row.requirement && req.acceptance_ids.contains(&row.acceptance))
        {
            structure.issues.push(format!(
                "覆盖矩阵引用不存在的 {} / {}",
                row.requirement, row.acceptance
            ));
        }
        if let Some(task) = structure.tasks.iter().find(|task| task.id == row.task) {
            if !task.requirement_refs.contains(&row.requirement)
                || !task.acceptance_refs.contains(&row.acceptance)
            {
                structure.issues.push(format!(
                    "覆盖矩阵与 {} 的 Requirement refs/Acceptance refs 不一致",
                    row.task
                ));
            }
        }
    }
    for task in &structure.tasks {
        for acceptance in &task.acceptance_refs {
            if !structure.coverage.iter().any(|row| {
                row.task == task.id
                    && row.acceptance == *acceptance
                    && task.requirement_refs.contains(&row.requirement)
            }) {
                structure
                    .issues
                    .push(format!("覆盖矩阵缺少 {} / {} 的声明", task.id, acceptance));
            }
        }
    }
}

fn unfinished_tasks(structure: &ChangeStructure) -> Vec<&TaskDoc> {
    if !structure.has_tasks {
        return Vec::new();
    }
    structure
        .tasks
        .iter()
        .filter(|task| {
            !matches!(
                task.status.to_ascii_lowercase().as_str(),
                "done" | "dropped"
            )
        })
        .collect()
}

fn has_dependency_cycle(graph: &BTreeMap<String, Vec<String>>) -> bool {
    fn visit(
        id: &str,
        graph: &BTreeMap<String, Vec<String>>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> bool {
        if visiting.contains(id) {
            return true;
        }
        if visited.contains(id) {
            return false;
        }
        visiting.insert(id.to_string());
        if let Some(children) = graph.get(id) {
            for child in children {
                if visit(child, graph, visiting, visited) {
                    return true;
                }
            }
        }
        visiting.remove(id);
        visited.insert(id.to_string());
        false
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    graph
        .keys()
        .any(|id| visit(id, graph, &mut visiting, &mut visited))
}

fn validate_verification(verification: &VerificationInput) -> Result<(), SddError> {
    if !matches!(
        verification.result.to_ascii_lowercase().as_str(),
        "pass" | "passed" | "fail" | "failed" | "needs_fix"
    ) {
        return Err(SddError::Invalid(format!(
            "SDD_INVALID_VERIFICATION_RESULT: {}",
            verification.result
        )));
    }
    if verification.checks.is_empty() {
        return Err(SddError::Invalid(
            "SDD_VERIFICATION_EVIDENCE_REQUIRED: at least one check is required when verification input is supplied.".into(),
        ));
    }
    let mut verification_ids = BTreeSet::new();
    for check in &verification.checks {
        if check.name.trim().is_empty() || check.status.trim().is_empty() {
            return Err(SddError::Invalid(
                "SDD_INVALID_VERIFICATION_CHECK: name and status are required.".into(),
            ));
        }
        if let Some(id) = &check.id {
            if !is_prefixed_id(id, "VERIFY-") {
                return Err(SddError::Invalid(format!(
                    "SDD_INVALID_VERIFICATION_ID: {}",
                    id
                )));
            }
            if !verification_ids.insert(id.clone()) {
                return Err(SddError::Invalid(format!("SDD_DUPLICATE_ID: {}", id)));
            }
        }
        let status = check.status.to_ascii_lowercase();
        if !matches!(
            status.as_str(),
            "pass" | "passed" | "fail" | "failed" | "skip" | "skipped" | "blocked"
        ) {
            return Err(SddError::Invalid(format!(
                "SDD_INVALID_VERIFICATION_STATUS: {}",
                check.status
            )));
        }
        for reference in &check.evidence_refs {
            if !is_prefixed_id(reference, "E-") {
                return Err(SddError::Invalid(format!(
                    "SDD_INVALID_EVIDENCE_REFERENCE: {}",
                    reference
                )));
            }
        }
        for reference in &check.requirement_refs {
            if !is_prefixed_id(reference, "REQ-") {
                return Err(SddError::Invalid(format!(
                    "SDD_INVALID_REQUIREMENT_REFERENCE: {}",
                    reference
                )));
            }
        }
        for reference in &check.acceptance_refs {
            if !is_prefixed_id(reference, "AC-") && !is_prefixed_id(reference, "ACC-") {
                return Err(SddError::Invalid(format!(
                    "SDD_INVALID_ACCEPTANCE_REFERENCE: {}",
                    reference
                )));
            }
        }
        for reference in &check.task_refs {
            if !is_prefixed_id(reference, "TASK-") {
                return Err(SddError::Invalid(format!(
                    "SDD_INVALID_TASK_REFERENCE: {}",
                    reference
                )));
            }
        }
    }
    let mut task_ids = BTreeSet::new();
    for task in &verification.tasks {
        if !is_prefixed_id(&task.id, "TASK-") {
            return Err(SddError::Invalid(format!(
                "SDD_INVALID_TASK_REFERENCE: {}",
                task.id
            )));
        }
        if !task_ids.insert(task.id.clone()) {
            return Err(SddError::Invalid(format!("SDD_DUPLICATE_ID: {}", task.id)));
        }
        if !matches!(
            task.status.to_ascii_lowercase().as_str(),
            "todo" | "doing" | "blocked" | "done" | "dropped"
        ) {
            return Err(SddError::Invalid(format!(
                "SDD_INVALID_TASK_STATUS: {}",
                task.status
            )));
        }
        for reference in &task.evidence_refs {
            if !is_prefixed_id(reference, "E-") {
                return Err(SddError::Invalid(format!(
                    "SDD_INVALID_EVIDENCE_REFERENCE: {}",
                    reference
                )));
            }
        }
    }
    Ok(())
}

fn inspect_verification(
    verification: &VerificationInput,
    structure: &ChangeStructure,
) -> Vec<String> {
    let mut issues = Vec::new();
    let requested_pass = matches!(
        verification.result.to_ascii_lowercase().as_str(),
        "pass" | "passed"
    );
    if requested_pass
        && verification.checks.iter().any(|check| {
            !matches!(
                check.status.to_ascii_lowercase().as_str(),
                "pass" | "passed"
            )
        })
    {
        issues.push("SDD_VERIFICATION_RESULT_MISMATCH: pass requires every check to pass.".into());
    }

    let requirements: BTreeSet<_> = structure
        .requirements
        .iter()
        .map(|requirement| requirement.id.as_str())
        .collect();
    let acceptances: BTreeSet<_> = structure
        .requirements
        .iter()
        .flat_map(|requirement| requirement.acceptance_ids.iter().map(String::as_str))
        .collect();
    let tasks: BTreeMap<_, _> = structure
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect();
    let snapshots: BTreeMap<_, _> = verification
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect();

    for task in &structure.tasks {
        match snapshots.get(task.id.as_str()) {
            Some(snapshot) if !snapshot.status.eq_ignore_ascii_case(&task.status) => {
                issues.push(format!(
                    "{} 状态快照为 {}，Markdown 当前状态为 {}",
                    task.id, snapshot.status, task.status
                ));
            }
            None => issues.push(format!("验证证据缺少 {} 状态快照", task.id)),
            Some(_) => {}
        }
    }
    for snapshot in &verification.tasks {
        if !tasks.contains_key(snapshot.id.as_str()) {
            issues.push(format!("状态快照引用不存在的 {}", snapshot.id));
        }
        for evidence in &snapshot.evidence_refs {
            if !structure.evidence_ids.contains(evidence) {
                issues.push(format!("{} 引用了未登记的证据 {}", snapshot.id, evidence));
            }
        }
    }

    for check in &verification.checks {
        let label = check.id.as_deref().unwrap_or(check.name.as_str());
        if check.acceptance_refs.is_empty() {
            issues.push(format!("{} 缺少 acceptanceRefs", label));
        }
        if structure.has_tasks && check.task_refs.is_empty() {
            issues.push(format!("{} 缺少 taskRefs", label));
        }
        for reference in &check.requirement_refs {
            if !requirements.contains(reference.as_str()) {
                issues.push(format!("{} 引用了不存在的 {}", label, reference));
            }
        }
        for reference in &check.acceptance_refs {
            if !acceptances.contains(reference.as_str()) {
                issues.push(format!("{} 引用了不存在的 {}", label, reference));
            }
        }
        for reference in &check.task_refs {
            if !tasks.contains_key(reference.as_str()) {
                issues.push(format!("{} 引用了不存在的 {}", label, reference));
            }
        }
        for reference in &check.evidence_refs {
            if !structure.evidence_ids.contains(reference) {
                issues.push(format!("{} 引用了未登记的证据 {}", label, reference));
            }
        }
    }
    if requested_pass {
        let checked_acceptances: BTreeSet<_> = verification
            .checks
            .iter()
            .flat_map(|check| check.acceptance_refs.iter().map(String::as_str))
            .collect();
        let checked_tasks: BTreeSet<_> = verification
            .checks
            .iter()
            .flat_map(|check| check.task_refs.iter().map(String::as_str))
            .collect();
        for requirement in &structure.requirements {
            if !requirement.priority.eq_ignore_ascii_case("must") {
                continue;
            }
            for acceptance in &requirement.acceptance_ids {
                if !checked_acceptances.contains(acceptance.as_str()) {
                    issues.push(format!("验证证据未覆盖 must 验收 {}", acceptance));
                }
            }
        }
        for task in &structure.tasks {
            if task.priority.eq_ignore_ascii_case("must")
                && !task.status.eq_ignore_ascii_case("dropped")
                && !checked_tasks.contains(task.id.as_str())
            {
                issues.push(format!("验证 checks 未引用 must 任务 {}", task.id));
            }
        }
    }
    issues
}

fn is_prefixed_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value[prefix.len()..]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

pub(crate) fn write_plain(path: &Path, body: &str) -> Result<(), SddError> {
    let temp = path.with_extension("tmp");
    fs::write(&temp, body).map_err(io_error)?;
    replace_file(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn new_creates_sdd_change_and_action_required() {
        let dir = tempdir().unwrap();
        let response = new(Some(dir.path().to_str().unwrap()), "实现订单取消功能").unwrap();
        let change = response.change.unwrap();
        assert_eq!(change.status, "draft");
        assert!(!dir.path().join(&change.path).join("spec").exists());
        assert!(!dir.path().join(&change.path).join("tasks").exists());
        let action = response.action_required.unwrap();
        assert_eq!(action.kind, "write_spec");
        assert!(action.path.ends_with(&change.id));
        assert!(action
            .instructions
            .iter()
            .any(|item| item.contains("spec/index.md")));
    }

    #[test]
    fn archive_moves_change_directory() {
        let dir = tempdir().unwrap();
        let response = new(Some(dir.path().to_str().unwrap()), "demo").unwrap();
        let id = response.change.unwrap().id;
        let change_dir = dir.path().join("sdd/changes").join(&id);
        fs::create_dir_all(change_dir.join("spec/requirements")).unwrap();
        fs::create_dir_all(change_dir.join("tasks")).unwrap();
        fs::write(change_dir.join("spec/index.md"), "# 规格\n").unwrap();
        fs::write(
            change_dir.join("spec/requirements/REQ-001.md"),
            "# REQ-001：demo\n\n- Priority: must\n- Status: proposed\n- Dependencies: 无\n\n## 验收条件\n\n### AC-001\n",
        )
        .unwrap();
        fs::write(
            change_dir.join("tasks/index.md"),
            "# 任务\n\n- Has tasks: false\n",
        )
        .unwrap();
        let verified = verify(Some(dir.path().to_str().unwrap()), &id, None).unwrap();
        assert_eq!(verified.change.unwrap().status, "verified");
        let moved = archive(Some(dir.path().to_str().unwrap()), &id).unwrap();
        assert_eq!(moved.change.unwrap().status, "archived");
        assert!(!change_dir.exists());
        assert!(dir.path().join("sdd/archives").join(&id).exists());
    }
}
