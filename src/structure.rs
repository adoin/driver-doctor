use crate::scan::{format_size, scan_directory, ScanEntry, ScanNode, ScanProgress};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const MAX_DOC_CHARS: usize = 48_000;
const MAJOR_ITEM_RATIO: f64 = 0.05;
const DEFAULT_VISIBLE_ITEMS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistributionKind {
    Small,
    Concentrated,
    Mixed,
    Dispersed,
}

#[derive(Clone, Debug)]
pub struct CompactedEntries {
    pub distribution: DistributionKind,
    pub cv: f64,
    pub top20_coverage: f64,
    pub visible: Vec<ScanEntry>,
    pub folded: Vec<ScanEntry>,
    pub folded_size: u64,
}

pub fn folded_label(count: usize, size: u64) -> String {
    format!("...{{{}个|{}}}", count, format_size(size))
}

pub fn compact_scan_entries(entries: &[ScanEntry]) -> CompactedEntries {
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let n = entries.len();
    let cv = coefficient_of_variation(entries);
    let top20_coverage = top_n_coverage(entries, DEFAULT_VISIBLE_ITEMS, total);

    if n <= DEFAULT_VISIBLE_ITEMS {
        return CompactedEntries {
            distribution: DistributionKind::Small,
            cv,
            top20_coverage,
            visible: entries.to_vec(),
            folded: Vec::new(),
            folded_size: 0,
        };
    }

    let distribution = if cv >= 1.5 && top20_coverage >= 0.75 {
        DistributionKind::Concentrated
    } else if cv < 1.0 || top20_coverage < 0.50 {
        DistributionKind::Dispersed
    } else {
        DistributionKind::Mixed
    };

    let mut ranked: Vec<(usize, u64)> = entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| (idx, entry.size))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut visible_indices = std::collections::HashSet::new();
    for (idx, _) in ranked.iter().take(DEFAULT_VISIBLE_ITEMS) {
        visible_indices.insert(*idx);
    }

    if matches!(
        distribution,
        DistributionKind::Concentrated | DistributionKind::Mixed
    ) && total > 0
    {
        for (idx, entry) in entries.iter().enumerate() {
            if entry.size as f64 / total as f64 >= MAJOR_ITEM_RATIO {
                visible_indices.insert(idx);
            }
        }
    }

    let mut visible = Vec::new();
    let mut folded = Vec::new();
    for (idx, entry) in entries.iter().enumerate() {
        if visible_indices.contains(&idx) {
            visible.push(entry.clone());
        } else {
            folded.push(entry.clone());
        }
    }
    let folded_size = folded.iter().map(|e| e.size).sum();

    CompactedEntries {
        distribution,
        cv,
        top20_coverage,
        visible,
        folded,
        folded_size,
    }
}

fn coefficient_of_variation(entries: &[ScanEntry]) -> f64 {
    if entries.is_empty() {
        return 0.0;
    }
    let n = entries.len() as f64;
    let mean = entries.iter().map(|e| e.size as f64).sum::<f64>() / n;
    if mean <= f64::EPSILON {
        return 0.0;
    }
    let variance = entries
        .iter()
        .map(|e| {
            let diff = e.size as f64 - mean;
            diff * diff
        })
        .sum::<f64>()
        / n;
    variance.sqrt() / mean
}

fn top_n_coverage(entries: &[ScanEntry], top_n: usize, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let mut sizes: Vec<u64> = entries.iter().map(|e| e.size).collect();
    sizes.sort_by(|a, b| b.cmp(a));
    sizes.into_iter().take(top_n).sum::<u64>() as f64 / total as f64
}

/// 将当前已扫描的一级列表格式化为 RAG 友好 Markdown。
pub fn format_current_level_rag(root: &Path, entries: &[ScanEntry]) -> String {
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let mut doc = format!(
        r#"# 目录结构快照（当前层级）

```yaml
type: directory_snapshot
root: "{}"
child_count: {}
aggregated_size: "{}"
scan_mode: current_level_only
```

## 直接子项明细

| 名称 | 类型 | 大小 | 占用空间 | 文件数 | 文件夹数 | 占比 | 路径 |
|------|------|------|----------|--------|----------|------|------|
"#,
        root.display(),
        entries.len(),
        format_size(total)
    );

    for e in entries {
        let kind = if e.is_dir { "folder" } else { "file" };
        doc.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {:.2}% | {} |\n",
            e.name.replace('|', "\\|"),
            kind,
            format_size(e.size),
            format_size(e.allocated),
            e.file_count,
            e.folder_count,
            e.percent,
            e.path.display()
        ));
    }

    doc.push_str("\n## 按大小 Top 子项（文本摘要）\n\n");
    for (i, e) in entries.iter().take(15).enumerate() {
        doc.push_str(&format!(
            "{}. [{}] {} — {} ({:.2}%)\n",
            i + 1,
            if e.is_dir { "文件夹" } else { "文件" },
            e.name,
            format_size(e.size),
            e.percent
        ));
    }

    truncate_doc(doc)
}

/// 深层扫描文件夹，生成多层级 RAG 文档（仅展开每层最大的若干目录）。
pub fn build_deep_structure_rag(
    root: &Path,
    max_depth: u32,
    top_n: usize,
    cancel: &Arc<AtomicBool>,
    on_progress: impl Fn(ScanProgress) + Send + Sync,
) -> Result<String, String> {
    if !root.is_dir() {
        return Err("只能对文件夹做深度结构分析".into());
    }

    let level0 = scan_directory(root, cancel, &on_progress)?;
    let total: u64 = level0.iter().map(|e| e.size).sum();

    let mut doc = format!(
        r#"# 目录深度结构报告

```yaml
type: deep_directory_tree
root: "{}"
max_depth: {max_depth}
top_n_per_level: {top_n}
aggregated_size: "{}"
```

## 层级 0 — 直接子项

| 名称 | 类型 | 大小 | 文件数 | 文件夹数 | 占比 | 路径 |
|------|------|------|--------|----------|------|------|
"#,
        root.display(),
        format_size(total)
    );

    for e in &level0 {
        append_table_row(&mut doc, e);
    }

    doc.push_str("\n## 深度树（按占用展开）\n\n```\n");
    doc.push_str(&format!("{}/ [{}]\n", root.display(), format_size(total)));

    let mut dirs: Vec<&ScanEntry> = level0.iter().filter(|e| e.is_dir).collect();
    dirs.sort_by(|a, b| b.size.cmp(&a.size));

    for entry in dirs.into_iter().take(top_n) {
        if cancel.load(Ordering::Relaxed) {
            return Err("扫描已取消".into());
        }
        append_tree_branch(&mut doc, entry, 1, max_depth, top_n, cancel, &on_progress)?;
        if doc.len() > MAX_DOC_CHARS {
            doc.push_str("\n... (输出已截断，层级过多)\n");
            break;
        }
    }

    doc.push_str("```\n");
    Ok(truncate_doc(doc))
}

fn append_table_row(doc: &mut String, e: &ScanEntry) {
    doc.push_str(&format!(
        "| {} | {} | {} | {} | {} | {:.2}% | {} |\n",
        e.name.replace('|', "\\|"),
        if e.is_dir { "folder" } else { "file" },
        format_size(e.size),
        e.file_count,
        e.folder_count,
        e.percent,
        e.path.display()
    ));
}

fn append_tree_branch(
    doc: &mut String,
    entry: &ScanEntry,
    depth: u32,
    max_depth: u32,
    top_n: usize,
    cancel: &Arc<AtomicBool>,
    on_progress: &(impl Fn(ScanProgress) + Send + Sync),
) -> Result<(), String> {
    let indent = "  ".repeat(depth as usize);
    doc.push_str(&format!(
        "{}{}/ — {} ({:.2}%)\n",
        indent,
        entry.name,
        format_size(entry.size),
        entry.percent
    ));

    if depth >= max_depth || doc.len() > MAX_DOC_CHARS {
        return Ok(());
    }

    let children = scan_directory(&entry.path, cancel, on_progress)?;
    let mut subdirs: Vec<&ScanEntry> = children.iter().filter(|e| e.is_dir).collect();
    subdirs.sort_by(|a, b| b.size.cmp(&a.size));

    for child in subdirs.into_iter().take(top_n) {
        append_tree_branch(doc, child, depth + 1, max_depth, top_n, cancel, on_progress)?;
    }

    // 大文件也列出前几个
    let mut files: Vec<&ScanEntry> = children.iter().filter(|e| !e.is_dir).collect();
    files.sort_by(|a, b| b.size.cmp(&a.size));
    for f in files.into_iter().take(5) {
        let fi = "  ".repeat((depth + 1) as usize);
        doc.push_str(&format!(
            "{}{} [file] — {}\n",
            fi,
            f.name,
            format_size(f.size)
        ));
    }

    Ok(())
}

fn truncate_doc(mut doc: String) -> String {
    if doc.len() > MAX_DOC_CHARS {
        doc.truncate(MAX_DOC_CHARS);
        doc.push_str("\n\n...(文档已截断以适配模型上下文)");
    }
    doc
}

/// 供 AI 使用的路径元信息块。
pub fn format_diagnostic_tree_rag(root: &Path, entries: &[ScanEntry], scan_mode: &str) -> String {
    let compacted = compact_scan_entries(entries);
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let mut doc = format!(
        r#"# Disk Diagnostic Tree

```yaml
type: disk_diagnostic_tree
root: "{}"
scan_mode: "{}"
item_count: {}
total_size: "{}"
coefficient_of_variation: {:.4}
top20_coverage: {:.4}
distribution: "{}"
folded_summary: "{}"
priority_rule: "设置转移 > 设置自动清理 > 设置关闭生成 > 卸载占用程序 > 暴力删除"
```

## major_items

"#,
        root.display(),
        scan_mode,
        entries.len(),
        format_size(total),
        compacted.cv,
        compacted.top20_coverage,
        distribution_name(compacted.distribution),
        folded_label(compacted.folded.len(), compacted.folded_size)
    );

    for (idx, entry) in compacted.visible.iter().enumerate() {
        doc.push_str(&format!(
            "- rank: {}\n  path: \"{}\"\n  name: \"{}\"\n  kind: {}\n  size: \"{}\"\n  size_bytes: {}\n  percent_of_scope: {:.4}\n  files: {}\n  folders: {}\n",
            idx + 1,
            entry.path.display(),
            entry.name.replace('"', "'"),
            if entry.is_dir { "folder" } else { "file" },
            format_size(entry.size),
            entry.size,
            if total > 0 {
                entry.size as f64 / total as f64
            } else {
                0.0
            },
            entry.file_count,
            entry.folder_count
        ));
    }

    doc.push_str("\n## folded_items\n\nfolded_items:\n");
    doc.push_str(&format!(
        "- label: \"{}\"\n  count: {}\n  size: \"{}\"\n  size_bytes: {}\n  reason: \"small_or_tail_items_folded_for_readability\"\n",
        folded_label(compacted.folded.len(), compacted.folded_size),
        compacted.folded.len(),
        format_size(compacted.folded_size),
        compacted.folded_size
    ));
    for entry in compacted.folded.iter().take(20) {
        doc.push_str(&format!(
            "  - sample_path: \"{}\"\n    sample_size: \"{}\"\n",
            entry.path.display(),
            format_size(entry.size)
        ));
    }

    truncate_doc(doc)
}

pub fn format_scan_tree_diagnostic_rag(root: &ScanNode, scan_mode: &str) -> String {
    let mut doc = format!(
        "# Full Scan Diagnostic Tree\n\n```yaml\ntype: disk_diagnostic_tree_full_scan\nroot: \"{}\"\nscan_mode: \"{}\"\nsource: \"single_recursive_scan_tree_cache\"\n```\n\n",
        root.entry.path.display(),
        scan_mode
    );
    let mut remaining_scopes = 24_usize;
    append_scan_node_scope(&mut doc, root, scan_mode, 0, 4, &mut remaining_scopes);
    truncate_doc(doc)
}

fn append_scan_node_scope(
    doc: &mut String,
    node: &ScanNode,
    scan_mode: &str,
    depth: u32,
    max_depth: u32,
    remaining_scopes: &mut usize,
) {
    if *remaining_scopes == 0 || depth > max_depth || doc.len() > MAX_DOC_CHARS {
        return;
    }
    *remaining_scopes -= 1;

    let entries: Vec<ScanEntry> = node
        .children
        .iter()
        .map(|child| child.entry.clone())
        .collect();
    doc.push_str(&format!(
        "\n\n## tree_scope depth={} path=\"{}\"\n\n",
        depth,
        node.entry.path.display()
    ));
    doc.push_str(&format_diagnostic_tree_rag(
        &node.entry.path,
        &entries,
        scan_mode,
    ));

    if depth >= max_depth {
        return;
    }

    let compacted = compact_scan_entries(&entries);
    let visible_paths: std::collections::HashSet<_> = compacted
        .visible
        .iter()
        .filter(|entry| entry.is_dir)
        .map(|entry| entry.path.clone())
        .collect();

    let mut child_dirs: Vec<&ScanNode> = node
        .children
        .iter()
        .filter(|child| child.entry.is_dir && visible_paths.contains(&child.entry.path))
        .collect();
    child_dirs.sort_by(|a, b| b.entry.size.cmp(&a.entry.size));

    for child in child_dirs.into_iter().take(8) {
        append_scan_node_scope(
            doc,
            child,
            scan_mode,
            depth + 1,
            max_depth,
            remaining_scopes,
        );
        if *remaining_scopes == 0 || doc.len() > MAX_DOC_CHARS {
            break;
        }
    }
}

fn distribution_name(kind: DistributionKind) -> &'static str {
    match kind {
        DistributionKind::Small => "small",
        DistributionKind::Concentrated => "concentrated",
        DistributionKind::Mixed => "mixed",
        DistributionKind::Dispersed => "dispersed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str, size: u64) -> ScanEntry {
        ScanEntry {
            name: name.into(),
            path: PathBuf::from(format!(r"C:\test\{name}")),
            is_dir: true,
            size,
            allocated: size,
            file_count: 1,
            folder_count: 0,
            percent: 0.0,
        }
    }

    #[test]
    fn folded_label_includes_count_and_size() {
        assert_eq!(folded_label(123, 50 * 1024 * 1024), "...{123个|50.00 MB}");
    }

    #[test]
    fn keeps_all_items_when_count_is_twenty_or_less() {
        let entries: Vec<ScanEntry> = (0..20).map(|i| entry(&format!("item-{i}"), 1024)).collect();

        let compacted = compact_scan_entries(&entries);

        assert_eq!(compacted.distribution, DistributionKind::Small);
        assert_eq!(compacted.visible.len(), 20);
        assert!(compacted.folded.is_empty());
    }

    #[test]
    fn concentrated_data_keeps_top_twenty_and_folds_tail() {
        let mut entries = vec![entry("huge", 900)];
        entries.extend((0..24).map(|i| entry(&format!("tiny-{i}"), 1)));

        let compacted = compact_scan_entries(&entries);

        assert_eq!(compacted.distribution, DistributionKind::Concentrated);
        assert_eq!(compacted.visible.len(), 20);
        assert_eq!(compacted.folded.len(), 5);
        assert!(compacted.cv >= 1.5);
        assert!(compacted.top20_coverage >= 0.75);
    }

    #[test]
    fn dispersed_data_keeps_top_twenty_for_visual_brevity() {
        let entries: Vec<ScanEntry> = (0..25).map(|i| entry(&format!("even-{i}"), 10)).collect();

        let compacted = compact_scan_entries(&entries);

        assert_eq!(compacted.distribution, DistributionKind::Dispersed);
        assert_eq!(compacted.visible.len(), 20);
        assert_eq!(compacted.folded.len(), 5);
        assert!(compacted.cv < 1.0);
    }

    #[test]
    fn diagnostic_tree_rag_includes_folded_summary_and_metrics() {
        let mut entries = vec![entry("huge", 900)];
        entries.extend((0..24).map(|i| entry(&format!("tiny-{i}"), 1)));

        let rag = format_diagnostic_tree_rag(Path::new(r"C:\"), &entries, "unit_test");

        assert!(rag.contains("type: disk_diagnostic_tree"));
        assert!(rag.contains("coefficient_of_variation:"));
        assert!(rag.contains("top20_coverage:"));
        assert!(rag.contains("...{5个|5 B}"));
        assert!(rag.contains("folded_items:"));
    }

    #[test]
    fn full_scan_tree_rag_includes_nested_scopes() {
        let leaf = ScanNode {
            entry: ScanEntry {
                name: "Cache".into(),
                path: Path::new(r"C:\Users\Admin\AppData\Local\Cache").to_path_buf(),
                is_dir: true,
                size: 60,
                allocated: 60,
                file_count: 3,
                folder_count: 0,
                percent: 60.0,
            },
            children: Vec::new(),
        };
        let appdata = ScanNode {
            entry: ScanEntry {
                name: "AppData".into(),
                path: Path::new(r"C:\Users\Admin\AppData").to_path_buf(),
                is_dir: true,
                size: 100,
                allocated: 100,
                file_count: 3,
                folder_count: 1,
                percent: 100.0,
            },
            children: vec![leaf],
        };
        let users = ScanNode {
            entry: ScanEntry {
                name: "Users".into(),
                path: Path::new(r"C:\Users").to_path_buf(),
                is_dir: true,
                size: 100,
                allocated: 100,
                file_count: 3,
                folder_count: 2,
                percent: 100.0,
            },
            children: vec![appdata],
        };
        let root = ScanNode {
            entry: ScanEntry {
                name: r"C:\".into(),
                path: Path::new(r"C:\").to_path_buf(),
                is_dir: true,
                size: 100,
                allocated: 100,
                file_count: 3,
                folder_count: 3,
                percent: 100.0,
            },
            children: vec![users],
        };

        let rag = format_scan_tree_diagnostic_rag(&root, "unit_full_tree");

        assert!(rag.contains("source: \"single_recursive_scan_tree_cache\""));
        assert!(rag.contains(r#"path="C:\Users""#));
        assert!(rag.contains(r#"path="C:\Users\Admin\AppData""#));
        assert!(rag.contains(r#"C:\Users\Admin\AppData\Local\Cache"#));
    }
}
