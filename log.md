# Change Log

## [2026-09-03]

- 全面重写 `README.md` 与 `README_CN.md`：统一产品定位、能力地图、快速上手、命令参考、Graph-RAG、Harness、SDD、配置、设计原则和开发验证说明，并保持中英文能力表述同步。

## [2026-09-03]

- 修改 `wiki sdd list`：默认只显示 `sdd/changes/`，新增 `--all` 显示归档变更；同步测试与文档。
- 新增 `wiki sdd list`：默认 YAML，`--json` 可选；合并列出 `sdd/changes/` 与 `sdd/archives/`，按 change-id 稳定排序并返回变更状态、阶段和路径。
- 增加空列表、活跃/归档混合列表的 CLI 回归测试；`cargo test --quiet` 通过 97 个单元测试、7 个集成测试组和 16 个 CLI 测试。

## [2026-09-02]

- 移除 `wiki sdd spec` 及规格 YAML/JSON 提交协议；`wiki sdd new` 现在只返回 Agent
  写入 `sdd/changes/<change-id>/spec/` 的路径和说明，规格由 Agent 直接维护 Markdown。
- `verify --input` 保留为验证证据入口，生命周期收敛为 new → Agent 写 spec → verify → archive。

- 参考 `E:/skills/sdd-driven-dev` 扩展 evidence-first SDD 规格：加入探索、证据链、决策、方案比较、影响分析、接口对齐、非代码改动、高危复核、发布护栏、测试策略和开放问题；全部渲染为 Markdown。
- 增加增强规格 CLI 回归测试、依赖 DAG 校验、`wiki init` 模板字节一致性断言，并同步 README/README_CN；`cargo test` 通过 108 个测试。
- 已按用户提供的 canonical `SDD.md` 同步根模板与 `docs/SDD.md`，并重建 `docs` 知识库索引；`cargo check`、`cargo test`、`ce doctor`、`ce check` 均通过。
- 修复 `wiki sdd verify` 在任务处于 `todo/doing/blocked` 时错误提示可归档的问题；现在验证返回 `fail/needs_fix`，并由 `archive` 复核所有未废弃任务必须为 `done/dropped`。
- 针对 Windows 归档 `os error 5` 增加目录移动退避重试和明确错误提示，避免直接暴露裸 `Access is denied`。
- 归档后 `change.md` 替换同样增加 Windows 可重试复制路径，减少文件被占用导致的半完成失败。

## [2026-08-28]

- Removed SDD design, plan, and build CLI stages; the supported lifecycle is now new → spec → verify → archive, with implementation handled by the project's normal engineering workflow.
- Rewrote SDD.md around the OpenSpec Explore stance: investigate first, keep open threads, visualize trade-offs, capture confirmed insights, and avoid implementation during exploration.
- Added the `spec` submission command and removed the SDD `design`/`plan`/`build` stages; regression coverage now verifies the reduced command surface and the new spec → verify → archive lifecycle.
- Final verification after the SDD simplification: cargo fmt/check passed and 95 tests passed; removed subcommands return an unrecognized-subcommand error.
- Synchronized the existing `docs/SDD.md` with the root SDD template used by `wiki init`; verified both files have identical content.
- Added the OpenSpec-inspired SDD guide as root `SDD.md`; `wiki init` now creates it and links it from generated `AGENTS.md` and `index.md`.
- Added the generated `SDD.md` workflow guide to `wiki init`, linked it from `AGENTS.md` and `index.md`, and covered its creation plus preservation behavior with init tests.

- Drafted the SDD extension plan for host-driven spec, design, plan, build, verify, and archive workflows.
- Recommended a filesystem-first `projects/changes/<change-id>/` layout, versioned JSON envelopes, `--input <PATH|->` handoff, explicit state transitions, and non-destructive archive-by-status for the MVP.
- Confirmed the existing Rust CLI has 85 passing tests and enough serde/time/atomic-file primitives to add SDD without a database migration.
- Revised per follow-up requirements: replace `inbox` with top-level `sdd/`, separate `sdd/changes/` and `sdd/archives/`, move a completed change directory during archive, and keep SDD artifact persistence in Markdown rather than multiple JSON files.
- Revised the SDD host protocol from JSON to YAML by default: `--yaml` is optional,
  `--json` selects JSON, host payloads are YAML via `--input <PATH|->`, and
  persisted artifacts remain Markdown only.
- Implemented the SDD MVP: added `wiki sdd` YAML commands, Markdown front matter/spec/design/plan/verify artifacts, build task progression, verify states, and changes-to-archives directory moves.
- Updated the standard wiki layout to remove `inbox` from new seeds and create `sdd/changes/` plus `sdd/archives/`; existing inbox directories remain untouched.
- Made SDD YAML the implicit default: `--yaml` is optional, `--json` is the explicit alternate, and the two flags are mutually exclusive; added five CLI regression tests, bringing the suite to 93 passing tests.
- Verified with `cargo fmt --check` and 93 passing tests; `ce doctor` handshake passed, while `ce scan` still reports repository size/complexity findings (including pre-existing baseline findings).

## [2026-08-25]

- Completed wiki rag ask configuration wiring.
- wiki rag ask now loads optional wiki.yaml from the executable directory.
- CLI values override wiki.yaml values; provider environment variables remain fallbacks.
- Added executable-directory loading coverage and fixed the default ask depth to 1.
- Verified with cargo fmt --check, cargo test (84 passed), and a CLI configuration smoke test.
- Fixed UTF-8 byte-boundary panic in entity line tracking; full tests now pass 85 cases.
- Verified against E:/sangou/sanguo: 308 pages indexed, 3461 chunks, no panic.
- Optimized first index builds by deferring orphan cleanup, skipping empty deletes for new pages, caching regexes, and making chunk sizing linear.
- Benchmarked E:/sangou/sanguo: rebuild dropped from about 60.1 seconds to 5.8 seconds; unchanged incremental indexing is about 0.18 seconds.

## [2026-09-02]

- Added wiki harness as the unified integration entry point for other projects.
- It auto-refreshes the rebuildable wiki index and returns bounded orientation plus task evidence in text, JSON, or downstream Prompt form.
- Added wiki.harness/v1 output metadata, source provenance, truncation/uncertainty flags, CLI regression tests, and generated-wiki guidance.
- Verified with cargo fmt --all, cargo test (100 tests), and ce check.
- Improved natural-language --query recall with exact-first retrieval and bounded technical-term/Chinese-fragment fallback; added query-term provenance and regression coverage.
- Restored internal SDD compatibility input structs required by the current renderer/check tests; no SDD command surface or index schema changed.
- Simplified Harness output by excluding the AGENTS.md Global context section and removing its unused agents_head_limit option; wiki read behavior is unchanged.
- Kept Harness overview focused on index/search navigation and task evidence by excluding recent logs and removing the unused log_last option; wiki read behavior is unchanged.
- Replaced Harness's static index.md catalog with query-matched Search index entries from direct lexical Graph-RAG hits; removed the now-unused index_head_limit option.
- Filtered root AGENTS.md and log.md from Harness Search index and task evidence even when query text matches them; wiki read/wiki rag behavior is unchanged.
- Fixed mixed ASCII/CJK fallback tokenization so queries such as index没有命中 can hit the Search index; verified against E:/sangou/sanguo with regression coverage.
- Clarified --format prompt so Search index is navigation-only and Sources maps only numbered Evidence; retrieval and JSON/text contracts are unchanged.
- Added explicit regex recall to Harness and RAG search/context/ask. Rust regex scans indexed chunks, bypasses literal fallback only when `--regex` is set, and carries `lexical/source=regex` into Search index and Evidence.
- Added regex success/error regression coverage; verified `EventStation|STATION_ING` against `E:/sangou/sanguo` with `uncertain=false`.
