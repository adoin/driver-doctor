use crate::scan::{format_size, list_drives_with_space};

/// 收集本机环境信息，注入 AI 提示词。
pub fn build_system_context(current_path: Option<&str>) -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "未知".into());
    let computer = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "未知".into());
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let mut ctx = format!(
        r#"[系统环境]
- 操作系统: Windows
- 用户名: {user}
- 计算机名: {computer}
- 当前时间: {now}
"#
    );

    if let Some(path) = current_path.filter(|p| !p.is_empty()) {
        ctx.push_str(&format!("- 当前浏览路径: {path}\n"));
    }

    let drives = list_drives_with_space();
    if !drives.is_empty() {
        ctx.push_str("- 磁盘概况:\n");
        for d in drives {
            ctx.push_str(&format!(
                "  · {}: 总 {} / 可用 {} ({:.0}% 已用)\n",
                d.letter,
                format_size(d.total_bytes),
                format_size(d.free_bytes),
                d.used_percent()
            ));
        }
    }

    ctx
}
