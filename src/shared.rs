//! Shared knowledge-base metadata and the default directory.
//!
//! Mirrors `fastctx/src/wiki/mod.rs` and `fastctx/src/control/knowledge.rs`
//! defaults: a knowledge base is rooted in `docs/` by default, and the
//! standard area set is `notes/`, `ideas/`, `projects/`, `sdd/` in that
//! semantic order.

use std::path::Path;

/// Default wiki root shared by `init`, `read`, and `search`.
pub const DEFAULT_WIKI_ROOT: &str = "docs";

/// Filename carrying global context at the wiki root.
pub const AGENTS_FILENAME: &str = "AGENTS.md";

/// Filename carrying the catalog and navigation aid at the wiki root.
pub const INDEX_FILENAME: &str = "index.md";

/// Filename carrying the spec-driven development workflow at the wiki root.
pub const SDD_FILENAME: &str = "SDD.md";

/// Append-only chronology parsed by `read --log-last` and `search`.
pub const LOG_FILENAME: &str = "log.md";

/// Top-level knowledge areas the seed creates and the read/search tools
/// count. The order encodes the semantic priority used by
/// `search --mode summary` (notes first because they are the verified
/// source of truth).
pub const KNOWLEDGE_AREAS: &[(&str, &str)] = &[
    ("notes", "verified source of truth"),
    ("ideas", "original reasoning and preferences"),
    ("projects", "active work and verification"),
    ("sdd", "spec-driven changes and archives"),
];

/// Returns the area that owns a relative path inside the wiki root, or
/// `None` for root documents and custom categories.
pub fn knowledge_area_for_relative_path(
    path: &Path,
) -> Option<&'static (&'static str, &'static str)> {
    let component = path.components().next()?.as_os_str().to_str()?;
    KNOWLEDGE_AREAS.iter().find(|area| area.0 == component)
}

/// Returns the canonical knowledge-area directory names.
pub fn knowledge_area_names() -> [&'static str; 4] {
    ["notes", "ideas", "projects", "sdd"]
}

/// Human-readable label for `search --mode summary` headings.
pub fn area_search_label(name: &str) -> &'static str {
    match name {
        "notes" => "Notes / source of truth",
        "ideas" => "Ideas / original reasoning",
        "projects" => "Projects / active work",
        "sdd" => "SDD / spec-driven changes",
        _ => "Knowledge area",
    }
}

/// Display a path with forward slashes for stable, human-readable output.
pub fn display_path(path: &Path) -> String {
    let mut value = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = value.strip_prefix("//?/UNC/") {
        value = format!("//{rest}");
    } else if let Some(rest) = value.strip_prefix("//?/") {
        value = rest.to_string();
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn knowledge_area_for_relative_path_matches_only_top_level_component() {
        let cases = [
            ("notes/architecture.md", Some("notes")),
            ("ideas/heuristics.md", Some("ideas")),
            ("projects/plan/phase.md", Some("projects")),
            ("sdd/changes/demo/spec/index.md", Some("sdd")),
            ("AGENTS.md", None),
            ("pages/custom.md", None),
        ];
        for (input, expected) in cases {
            let got = knowledge_area_for_relative_path(&PathBuf::from(input)).map(|area| area.0);
            assert_eq!(got, expected, "input: {input}");
        }
    }

    use std::path::Path;
    #[test]
    fn display_path_strips_windows_extended_prefix() {
        let value = display_path(Path::new("C:/Users/me/notes.md"));
        assert!(value.contains("/notes.md"), "{value}");
    }

    #[test]
    fn area_search_label_handles_known_areas_and_unknown_input() {
        assert_eq!(area_search_label("notes"), "Notes / source of truth");
        assert_eq!(area_search_label("ideas"), "Ideas / original reasoning");
        assert_eq!(area_search_label("projects"), "Projects / active work");
        assert_eq!(area_search_label("sdd"), "SDD / spec-driven changes");
        assert_eq!(area_search_label("archive"), "Knowledge area");
    }
}
