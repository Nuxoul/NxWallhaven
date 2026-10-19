//! Application-owned persistent WebView2 data; never imports system-browser cookies.
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
static COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn saved_profile_path() -> Result<PathBuf, String> {
    if let Some(base) = std::env::var_os("WH_STUDIO_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(base).join("webview-profile"));
    }
    Ok(local_app_data()?.join("WallHavenStudio").join("WebView2"))
}
fn local_app_data() -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
    };
    let mut value = std::ptr::null_mut();
    let status = unsafe {
        SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, std::ptr::null_mut(), &mut value)
    };
    if status < 0 || value.is_null() {
        return Err("无法确定本地浏览器资料目录".into());
    }
    let mut len = 0;
    while unsafe { *value.add(len) } != 0 {
        len += 1;
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(unsafe {
        std::slice::from_raw_parts(value, len)
    }));
    unsafe {
        CoTaskMemFree(value.cast());
    }
    if path.is_absolute() {
        Ok(path)
    } else {
        Err("无法确定本地浏览器资料目录".into())
    }
}
pub(super) fn has_saved_profile() -> bool {
    saved_profile_path().is_ok_and(|path| path.join("session-profile-v1").is_file())
}
pub(super) struct BrowserProfile {
    pub path: PathBuf,
    temporary: bool,
    lease: Option<File>,
}
impl BrowserProfile {
    pub fn persistent(path: PathBuf) -> Result<Self, String> {
        Self::open(path, false)
    }
    #[cfg(test)]
    pub fn temporary() -> Result<Self, String> {
        Self::open(
            std::env::temp_dir().join(format!(
                "wh-webview-test-{}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::AcqRel),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )),
            true,
        )
    }
    fn open(path: PathBuf, temporary: bool) -> Result<Self, String> {
        std::fs::create_dir_all(&path).map_err(|_| "无法创建应用浏览器资料目录")?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let lease = options
            .open(path.join("session.lock"))
            .map_err(|_| "浏览器资料正在被另一个程序窗口使用，或目录不可写")?;
        Ok(Self {
            path,
            temporary,
            lease: Some(lease),
        })
    }
    pub fn mark_initialized(&self) -> Result<(), String> {
        std::fs::write(
            self.path.join("session-profile-v1"),
            b"WallHaven Studio WebView2 profile v1\n",
        )
        .map_err(|_| "无法保存浏览器会话标记".into())
    }
    pub fn forget_marker(&self) -> Result<(), String> {
        match std::fs::remove_file(self.path.join("session-profile-v1")) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("无法清除浏览器恢复标记".into()),
        }
    }
}
impl Drop for BrowserProfile {
    fn drop(&mut self) {
        self.lease.take();
        if self.temporary {
            for _ in 0..10 {
                if std::fs::remove_dir_all(&self.path).is_ok() || !self.path.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_app_data_uses_windows_known_folder_without_environment() {
        let path = local_app_data().unwrap();
        assert!(path.is_absolute());
        assert!(path.is_dir());
    }
    #[test]
    fn persistent_profile_survives_close_and_excludes_second_writer() {
        let dir = std::env::temp_dir().join(format!(
            "wh-profile-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::AcqRel)
        ));
        let profile = BrowserProfile::persistent(dir.clone()).unwrap();
        profile.mark_initialized().unwrap();
        assert!(BrowserProfile::persistent(dir.clone()).is_err());
        drop(profile);
        assert!(dir.join("session-profile-v1").exists());
        let again = BrowserProfile::persistent(dir.clone()).unwrap();
        again.forget_marker().unwrap();
        assert!(!dir.join("session-profile-v1").exists());
        drop(again);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
