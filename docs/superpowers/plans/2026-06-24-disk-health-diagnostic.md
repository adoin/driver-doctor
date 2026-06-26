# Disk Health Diagnostic Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add smart small-item folding plus a disk health diagnostic report that gives AI a clear hierarchy of major folders, major files, and folded small items.

**Architecture:** Keep scan data unchanged, add pure compaction helpers in `src/structure.rs`, and let the UI render virtual folded rows from existing `ScanEntry` values. Add a new AI prompt and right-click drive action that builds a diagnostic tree RAG document before calling the configured chat model.

**Tech Stack:** Rust, eframe/egui, existing jwalk scanner, existing OpenAI-compatible chat client.

---

### Task 1: Smart Folding Rules

**Files:**
- Modify: `src/structure.rs`

- [ ] Add unit tests for `CV + top20_coverage` classification and folded label formatting.
- [ ] Run `cargo test structure::tests::` and verify tests fail before implementation.
- [ ] Implement `compact_scan_entries`, `folded_label`, and distribution metadata.
- [ ] Run `cargo test structure::tests::` and verify tests pass.

### Task 2: Folded Rows In UI

**Files:**
- Modify: `src/app.rs`

- [ ] Add a `FileListRow` enum for real entries and virtual folded groups.
- [ ] Track expanded folded groups by current path.
- [ ] Render folded rows as `...{123个|50 MB}` and expand/collapse on click.
- [ ] Keep double-click behavior for real rows unchanged.

### Task 3: Diagnostic RAG And Health Check

**Files:**
- Modify: `src/structure.rs`
- Modify: `src/ai.rs`
- Modify: `src/app.rs`

- [ ] Generate `disk_diagnostic_tree` data with path hierarchy, CV, top20 coverage, major items, and folded summaries.
- [ ] Add `generate_health_check_report` with prompt sections: `占用报告`, `清理意见报告`, and priority order `设置转移 > 设置自动清理 > 设置关闭生成 > 卸载占用程序 > 暴力删除`.
- [ ] Add drive context menu action `🩺健康检查`.
- [ ] Run `cargo test` and `cargo check`.
