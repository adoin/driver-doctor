//! Windows Shell 图标（与资源管理器相同来源）

use egui::{ColorImage, TextureHandle, TextureOptions, Ui, Vec2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const ICON_PX: i32 = 16;

pub struct ShellIconCache {
    textures: HashMap<String, TextureHandle>,
}

impl Default for ShellIconCache {
    fn default() -> Self {
        Self {
            textures: HashMap::new(),
        }
    }
}

impl ShellIconCache {
    pub fn icon_size() -> Vec2 {
        Vec2::new(18.0, 18.0)
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        path: &Path,
        is_dir: bool,
        fallback_drive: bool,
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(Self::icon_size(), egui::Sense::hover());
        let key = cache_key(path, is_dir);

        if !self.textures.contains_key(&key) {
            if let Some(image) = load_shell_icon(path, is_dir) {
                let name = format!("shell_icon_{key}");
                let tex = ui.ctx().load_texture(name, image, TextureOptions::LINEAR);
                self.textures.insert(key.clone(), tex);
            }
        }

        if let Some(tex) = self.textures.get(&key) {
            ui.painter().image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            crate::icons::paint_fallback_icon(ui.painter(), rect, is_dir, fallback_drive);
        }

        response
    }

    /// 限制缓存体积，避免长时间浏览占用过多显存
    pub fn trim(&mut self, max_entries: usize) {
        if self.textures.len() > max_entries {
            self.textures.clear();
        }
    }
}

fn cache_key(path: &Path, _is_dir: bool) -> String {
    path.display().to_string()
}

#[cfg(windows)]
fn load_shell_icon(path: &Path, is_dir: bool) -> Option<ColorImage> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use winapi::shared::windef::HICON;
    use winapi::um::shellapi::{
        SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES,
    };
    use winapi::um::winnt::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
    use winapi::um::winuser::DestroyIcon;

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let attrs = if is_dir {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };

    let mut shfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        SHGetFileInfoW(
            wide.as_ptr(),
            attrs,
            &mut shfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        )
    };
    if ok == 0 || shfi.hIcon.is_null() {
        return None;
    }

    let image = unsafe { hicon_to_color_image(shfi.hIcon as HICON) };
    unsafe { DestroyIcon(shfi.hIcon as HICON) };
    image
}

#[cfg(windows)]
unsafe fn hicon_to_color_image(hicon: winapi::shared::windef::HICON) -> Option<ColorImage> {
    use std::mem::zeroed;
    use std::ptr::null_mut;
    use winapi::um::wingdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use winapi::um::winuser::{DrawIconEx, GetDC, ReleaseDC};
    const DI_NORMAL: u32 = 0x0003;

    let hdc_screen = GetDC(null_mut());
    if hdc_screen.is_null() {
        return None;
    }
    let hdc_mem = CreateCompatibleDC(hdc_screen);
    if hdc_mem.is_null() {
        ReleaseDC(null_mut(), hdc_screen);
        return None;
    }

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: ICON_PX,
            biHeight: -ICON_PX,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        },
        ..zeroed()
    };

    let mut bits: *mut winapi::ctypes::c_void = null_mut();
    let hbmp = CreateDIBSection(hdc_mem, &mut bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if hbmp.is_null() || bits.is_null() {
        DeleteDC(hdc_mem);
        ReleaseDC(null_mut(), hdc_screen);
        return None;
    }

    SelectObject(hdc_mem, hbmp as _);
    if DrawIconEx(
        hdc_mem,
        0,
        0,
        hicon,
        ICON_PX,
        ICON_PX,
        0,
        null_mut(),
        DI_NORMAL,
    ) == 0
    {
        DeleteObject(hbmp as _);
        DeleteDC(hdc_mem);
        ReleaseDC(null_mut(), hdc_screen);
        return None;
    }

    let len = (ICON_PX * ICON_PX * 4) as usize;
    let mut rgba = vec![0u8; len];
    std::ptr::copy_nonoverlapping(bits as *const u8, rgba.as_mut_ptr(), len);

    DeleteObject(hbmp as _);
    DeleteDC(hdc_mem);
    ReleaseDC(null_mut(), hdc_screen);

    // GDI 为 BGRA，转为 RGBA；透明处 alpha 可能为 0
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }

    Some(ColorImage::from_rgba_unmultiplied(
        [ICON_PX as usize, ICON_PX as usize],
        &rgba,
    ))
}

#[cfg(not(windows))]
fn load_shell_icon(_path: &Path, _is_dir: bool) -> Option<ColorImage> {
    None
}

/// 磁盘根路径
pub fn drive_icon_path(letter: &str) -> PathBuf {
    PathBuf::from(format!("{}:\\", letter.trim_end_matches(':')))
}
