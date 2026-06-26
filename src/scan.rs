use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

#[derive(Clone, Debug, Default)]
pub struct TreeStats {
    pub size: u64,
    pub allocated: u64,
    pub file_count: u64,
}

#[derive(Clone, Debug)]
pub struct ScanEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub allocated: u64,
    pub file_count: u64,
    pub folder_count: u64,
    pub percent: f64,
}

#[derive(Clone, Debug)]
pub struct ScanProgress {
    pub scanned_files: u64,
    pub current_path: String,
}

#[derive(Clone, Debug)]
pub struct ScanNode {
    pub entry: ScanEntry,
    pub children: Vec<ScanNode>,
}

#[derive(Clone, Debug)]
pub struct ScanResult {
    pub entries: Vec<ScanEntry>,
    pub tree: Option<ScanNode>,
}

struct ProgressGuard {
    counter: AtomicU64,
    last: Mutex<Instant>,
}

impl ProgressGuard {
    fn new() -> Self {
        Self {
            counter: AtomicU64::new(0),
            last: Mutex::new(Instant::now()),
        }
    }

    fn report(&self, path: &str, on_progress: &(impl Fn(ScanProgress) + Send + Sync)) {
        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let mut last = self.last.lock().unwrap();
        if n == 1 || last.elapsed().as_millis() >= 120 || n % 800 == 0 {
            on_progress(ScanProgress {
                scanned_files: n,
                current_path: path.to_string(),
            });
            *last = Instant::now();
        }
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.2} {}", size, UNITS[unit])
    }
}

#[allow(dead_code)]
pub fn format_datetime(t: Option<SystemTime>) -> String {
    t.map(format_system_time).unwrap_or_else(|| "-".into())
}

#[allow(dead_code)]
pub fn format_datetime_full(t: Option<SystemTime>) -> String {
    t.map(|st| {
        if let Ok(d) = st.duration_since(std::time::UNIX_EPOCH) {
            if let Some(dt) = chrono::DateTime::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
            {
                return dt
                    .with_timezone(&chrono::Local)
                    .format("%Y.%m.%d %H:%M:%S")
                    .to_string();
            }
        }
        format_system_time(st)
    })
    .unwrap_or_else(|| "-".into())
}

fn format_system_time(st: SystemTime) -> String {
    match st.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => chrono::DateTime::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
            .map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%Y.%m.%d")
                    .to_string()
            })
            .unwrap_or_else(|| "-".into()),
        Err(_) => "-".into(),
    }
}

#[allow(dead_code)]
fn later(a: Option<SystemTime>, b: Option<SystemTime>) -> Option<SystemTime> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if x > y { x } else { y }),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn allocated_file_size(len: u64, cluster: u64) -> u64 {
    if len == 0 {
        return 0;
    }
    if cluster == 0 {
        return len;
    }
    ((len + cluster - 1) / cluster) * cluster
}

#[cfg(windows)]
fn disk_cluster_size(path: &Path) -> u64 {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use winapi::um::fileapi::GetDiskFreeSpaceW;

    let root = drive_root(path);
    let wide: Vec<u16> = OsStr::new(&root)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut sectors = 0;
    let mut bytes = 0;
    unsafe {
        if GetDiskFreeSpaceW(
            wide.as_ptr(),
            &mut sectors,
            &mut bytes,
            ptr::null_mut(),
            ptr::null_mut(),
        ) != 0
        {
            return (sectors as u64) * (bytes as u64);
        }
    }
    4096
}

#[cfg(not(windows))]
fn disk_cluster_size(_path: &Path) -> u64 {
    4096
}

fn drive_root(path: &Path) -> String {
    let s = path.display().to_string();
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        format!("{}:\\", s.as_bytes()[0] as char)
    } else {
        "C:\\".into()
    }
}

#[allow(dead_code)]
fn file_owner(_path: &Path) -> String {
    std::env::var("USERNAME").unwrap_or_else(|_| "-".into())
}

fn stats_from_metadata(meta: &fs::Metadata, cluster: u64, is_dir: bool) -> TreeStats {
    let len = meta.len();
    TreeStats {
        size: len,
        allocated: if is_dir {
            0
        } else {
            allocated_file_size(len, cluster)
        },
        file_count: if is_dir { 0 } else { 1 },
    }
}

fn empty_scan_node(path: &Path, name: String) -> ScanNode {
    ScanNode {
        entry: ScanEntry {
            name,
            path: path.to_path_buf(),
            is_dir: path.is_dir(),
            size: 0,
            allocated: 0,
            file_count: 0,
            folder_count: 0,
            percent: 0.0,
        },
        children: Vec::new(),
    }
}

fn apply_percentages(entries: &mut [ScanEntry]) {
    let total: u64 = entries.iter().map(|e| e.size).sum();
    for e in entries.iter_mut() {
        e.percent = if total > 0 {
            e.size as f64 / total as f64 * 100.0
        } else {
            0.0
        };
    }
}

fn apply_child_percentages(children: &mut [ScanNode]) {
    let total: u64 = children.iter().map(|child| child.entry.size).sum();
    for child in children.iter_mut() {
        child.entry.percent = if total > 0 {
            child.entry.size as f64 / total as f64 * 100.0
        } else {
            0.0
        };
    }
}

fn scan_node(
    path: &Path,
    name: String,
    cancel: &Arc<AtomicBool>,
    cluster: u64,
    progress: &ProgressGuard,
    on_progress: &(impl Fn(ScanProgress) + Send + Sync),
) -> ScanNode {
    if cancel.load(Ordering::Relaxed) {
        return empty_scan_node(path, name);
    }

    progress.report(&path.display().to_string(), on_progress);
    let meta = fs::metadata(path).ok();
    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);

    if !is_dir {
        let stats = meta
            .as_ref()
            .map(|m| stats_from_metadata(m, cluster, false))
            .unwrap_or_default();
        return ScanNode {
            entry: ScanEntry {
                name,
                path: path.to_path_buf(),
                is_dir: false,
                size: stats.size,
                allocated: stats.allocated,
                file_count: stats.file_count,
                folder_count: 0,
                percent: 0.0,
            },
            children: Vec::new(),
        };
    }

    let child_paths: Vec<(PathBuf, String)> = fs::read_dir(path)
        .ok()
        .into_iter()
        .flat_map(|read_dir| read_dir.filter_map(|item| item.ok()))
        .map(|item| (item.path(), item.file_name().to_string_lossy().into_owned()))
        .collect();

    let mut children: Vec<ScanNode> = child_paths
        .par_iter()
        .map(|(child_path, child_name)| {
            scan_node(
                child_path,
                child_name.clone(),
                cancel,
                cluster,
                progress,
                on_progress,
            )
        })
        .collect();
    children.sort_by(|a, b| b.entry.size.cmp(&a.entry.size));
    apply_child_percentages(&mut children);

    let mut size = 0_u64;
    let mut allocated = 0_u64;
    let mut file_count = 0_u64;
    let mut folder_count = 0_u64;
    for child in &children {
        size += child.entry.size;
        allocated += child.entry.allocated;
        file_count += child.entry.file_count;
        if child.entry.is_dir {
            folder_count += 1 + child.entry.folder_count;
        }
    }

    ScanNode {
        entry: ScanEntry {
            name,
            path: path.to_path_buf(),
            is_dir: true,
            size,
            allocated,
            file_count,
            folder_count,
            percent: 0.0,
        },
        children,
    }
}

pub fn scan_directory_with_tree(
    root: &Path,
    cancel: &Arc<AtomicBool>,
    on_progress: impl Fn(ScanProgress) + Send + Sync,
) -> Result<ScanResult, String> {
    if !root.exists() {
        return Err(format!("路径不存在: {}", root.display()));
    }

    let cluster = disk_cluster_size(root);
    let progress = ProgressGuard::new();
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let tree = scan_node(root, name, cancel, cluster, &progress, &on_progress);
    if cancel.load(Ordering::Relaxed) {
        return Err("扫描已取消".into());
    }

    let entries = if tree.entry.is_dir {
        tree.children
            .iter()
            .map(|child| child.entry.clone())
            .collect()
    } else {
        vec![tree.entry.clone()]
    };

    Ok(ScanResult {
        entries,
        tree: Some(tree),
    })
}

pub fn scan_directory(
    root: &Path,
    cancel: &Arc<AtomicBool>,
    on_progress: impl Fn(ScanProgress) + Send + Sync,
) -> Result<Vec<ScanEntry>, String> {
    scan_directory_with_tree(root, cancel, on_progress).map(|result| result.entries)
}

pub fn scan_all_drives(
    cancel: &Arc<AtomicBool>,
    on_progress: impl Fn(ScanProgress) + Send + Sync,
    top_n: usize,
) -> Result<Vec<ScanEntry>, String> {
    let drives = list_windows_drives();
    if drives.is_empty() {
        return Err("未找到可用磁盘".into());
    }

    let on_progress = Arc::new(on_progress);
    let progress = ProgressGuard::new();

    let tasks: Vec<(PathBuf, String)> = drives
        .iter()
        .flat_map(|drive| {
            fs::read_dir(drive)
                .ok()
                .into_iter()
                .flat_map(|rd| rd.flatten())
                .filter_map(|item| {
                    let path = item.path();
                    if !path.is_dir() {
                        return None;
                    }
                    let name = format!(
                        "[{}] {}",
                        drive.trim_end_matches('\\'),
                        item.file_name().to_string_lossy()
                    );
                    Some((path, name))
                })
        })
        .collect();

    let mut all_entries: Vec<ScanEntry> = tasks
        .par_iter()
        .filter_map(|(path, name)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let cluster = disk_cluster_size(path);
            Some(
                scan_node(
                    path,
                    name.clone(),
                    cancel,
                    cluster,
                    &progress,
                    &*on_progress,
                )
                .entry,
            )
        })
        .collect();

    all_entries.sort_by(|a, b| b.size.cmp(&a.size));
    all_entries.truncate(top_n);
    apply_percentages(&mut all_entries);
    Ok(all_entries)
}

fn list_windows_drives() -> Vec<String> {
    (b'A'..=b'Z')
        .map(|c| format!("{}:\\", c as char))
        .filter(|d| Path::new(d).exists())
        .collect()
}

#[derive(Clone, Debug)]
pub struct DriveInfo {
    pub letter: String,
    pub path: PathBuf,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

impl DriveInfo {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    pub fn used_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            self.used_bytes() as f64 / self.total_bytes as f64 * 100.0
        }
    }

    pub fn label(&self) -> String {
        format!("本地磁盘 ({})", self.letter)
    }

    pub fn to_entry(&self) -> ScanEntry {
        ScanEntry {
            name: self.label(),
            path: self.path.clone(),
            is_dir: true,
            size: self.used_bytes(),
            allocated: self.total_bytes,
            file_count: 0,
            folder_count: 0,
            percent: self.used_percent(),
        }
    }
}

pub fn quick_list_directory(root: &Path) -> Result<Vec<ScanEntry>, String> {
    if !root.is_dir() {
        return Err(format!("不是文件夹: {}", root.display()));
    }

    let mut entries: Vec<ScanEntry> = fs::read_dir(root)
        .map_err(|e| format!("无法读取目录: {e}"))?
        .filter_map(|item| item.ok())
        .map(|item| {
            let path = item.path();
            let meta = item.metadata().ok();
            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            ScanEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                path: path.clone(),
                is_dir,
                size: 0,
                allocated: 0,
                file_count: 0,
                folder_count: 0,
                percent: 0.0,
            }
        })
        .collect();

    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(entries)
}

pub fn list_drives_with_space() -> Vec<DriveInfo> {
    list_windows_drives()
        .into_iter()
        .filter_map(|d| {
            let path = PathBuf::from(&d);
            let total = fs2::total_space(&path).ok()?;
            let free = fs2::available_space(&path).ok()?;
            Some(DriveInfo {
                letter: d.trim_end_matches('\\').to_string(),
                path,
                total_bytes: total,
                free_bytes: free,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_tree_result_exposes_nested_entries() {
        let root = std::env::temp_dir().join(format!(
            "driver_doctor_scan_tree_test_{}",
            std::process::id()
        ));
        let nested = root.join("Users").join("Admin").join("AppData");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("cache.bin"), [1_u8; 16]).unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let result = scan_directory_with_tree(&root, &cancel, |_| {}).unwrap();

        let tree = result.tree.unwrap();
        assert!(result.entries.iter().any(|entry| entry.name == "Users"));
        let users = tree
            .children
            .iter()
            .find(|node| node.entry.name == "Users")
            .unwrap();
        let admin = users
            .children
            .iter()
            .find(|node| node.entry.name == "Admin")
            .unwrap();
        assert!(admin
            .children
            .iter()
            .any(|node| node.entry.name == "AppData"));

        let _ = fs::remove_dir_all(root);
    }
}
