//! 配置持久化
//!
//! 存放位置：`%APPDATA%\WallHavenStudio\config.json`
//!
//! API Key 只在内存中保留明文；配置文件只保存绑定当前 Windows 用户的
//! DPAPI 密文。DPAPI 失败时返回错误，绝不回退为明文持久化。

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// 仅供运行时使用，永不直接序列化。
    pub api_key: Option<String>,

    /// 与当前内存 key 对应的 DPAPI 密文，永不包含明文。
    api_key_protected: Option<String>,

    pub download_dir: Option<String>,

    /// 0 = 瀑布流, 1 = 统一网格
    pub layout_mode: i32,

    pub nsfw_enabled: bool,

    /// Collection URLs require a username; the settings API does not return it.
    pub account_username: Option<String>,
}

/// 唯一用于写入磁盘的 DTO。它没有明文 API key 字段。
#[derive(Debug, Serialize)]
struct PersistedSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key_protected: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    download_dir: Option<String>,

    layout_mode: i32,
    nsfw_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_username: Option<String>,
}

/// 读取时兼容旧版明文字段；该类型永远不用于序列化。
#[derive(Debug, Default, Deserialize)]
struct SettingsInput {
    #[serde(default)]
    api_key_protected: Option<String>,

    #[serde(default)]
    api_key: Option<String>,

    #[serde(default)]
    download_dir: Option<String>,

    #[serde(default)]
    layout_mode: i32,

    #[serde(default)]
    nsfw_enabled: bool,

    #[serde(default)]
    account_username: Option<String>,
}

impl Settings {
    pub fn config_path() -> PathBuf {
        if let Some(base) = std::env::var_os("WH_STUDIO_CONFIG_DIR") {
            if !base.is_empty() {
                return PathBuf::from(base).join("config.json");
            }
        }

        #[cfg(windows)]
        {
            let base = std::env::var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."));
            base.join("WallHavenStudio").join("config.json")
        }
        #[cfg(not(windows))]
        {
            let base = std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."));
            base.join(".config")
                .join("wallhaven-studio")
                .join("config.json")
        }
    }

    /// 保持原有的容错加载 API。错误时返回无密钥的默认设置，且日志不包含配置内容。
    pub fn load() -> Self {
        match Self::try_load() {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("Failed to load configuration: {error}");
                Self::default()
            }
        }
    }

    /// 可检查错误的加载 API，包括 DPAPI 解密错误。
    pub fn try_load() -> Result<Self, String> {
        Self::load_from_path(&Self::config_path())
    }

    fn load_from_path(path: &Path) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("could not read config: {error}")),
        };

        Self::from_json(&text)
    }

    fn from_json(text: &str) -> Result<Self, String> {
        let text = text.trim_start_matches('\u{feff}');
        let input: SettingsInput = serde_json::from_str(text).map_err(|error| {
            format!(
                "invalid config JSON at line {}, column {}",
                error.line(),
                error.column()
            )
        })?;

        // An encrypted value always wins. In particular, corrupt ciphertext must not
        // silently fall back to a legacy plaintext value in the same file.
        let (api_key, api_key_protected) = match input.api_key_protected {
            Some(encrypted) => {
                let plain = dpapi::unprotect_b64(&encrypted)
                    .map_err(|error| format!("could not decrypt API key: {error}"))?;
                if plain.is_empty() {
                    (None, None)
                } else {
                    (Some(plain), Some(encrypted))
                }
            }
            None => {
                let legacy = input.api_key.filter(|key| !key.is_empty());
                (legacy, None)
            }
        };

        Ok(Self {
            api_key,
            api_key_protected,
            download_dir: input.download_dir,
            layout_mode: input.layout_mode,
            nsfw_enabled: input.nsfw_enabled,
            account_username: input.account_username,
        })
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to_path(&Self::config_path())
    }

    fn save_to_path(&self, path: &Path) -> Result<(), String> {
        let api_key_protected = self.protected_key_for_save()?;
        let persisted = PersistedSettings {
            api_key_protected,
            download_dir: self.download_dir.clone(),
            layout_mode: self.layout_mode,
            nsfw_enabled: self.nsfw_enabled,
            account_username: self.account_username.clone(),
        };
        let text = serde_json::to_string_pretty(&persisted)
            .map_err(|error| format!("could not serialize config: {error}"))?;

        atomic_write(path, text.as_bytes())
    }

    fn protected_key_for_save(&self) -> Result<Option<String>, String> {
        let Some(plain) = self.api_key.as_deref().filter(|key| !key.is_empty()) else {
            return Ok(None);
        };

        // Reuse a verified ciphertext when possible. If public runtime state was
        // changed directly, protect the new value instead of persisting stale data.
        if let Some(encrypted) = &self.api_key_protected {
            let protected_plain = dpapi::unprotect_b64(encrypted)
                .map_err(|error| format!("could not verify protected API key: {error}"))?;
            if protected_plain == plain {
                return Ok(Some(encrypted.clone()));
            }
        }

        dpapi::protect_b64(plain)
            .map(Some)
            .map_err(|error| format!("could not protect API key: {error}"))
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key
            .as_deref()
            .map(|key| !key.is_empty())
            .unwrap_or(false)
    }

    /// 设置 API Key。签名：`set_api_key(&mut self, Option<String>) -> Result<(), String>`。
    ///
    /// 非空 key 必须先成功通过 DPAPI 保护，之后才会提交到当前设置；因此失败时
    /// 原有 key 和密文都保持不变。`None` 或空字符串会清除 key 和密文。
    pub fn set_api_key(&mut self, key: Option<String>) -> Result<(), String> {
        match key {
            Some(key) if !key.is_empty() => {
                let encrypted = dpapi::protect_b64(&key)
                    .map_err(|error| format!("could not protect API key: {error}"))?;
                self.api_key = Some(key);
                self.api_key_protected = Some(encrypted);
                Ok(())
            }
            _ => {
                self.api_key = None;
                self.api_key_protected = None;
                Ok(())
            }
        }
    }

    pub fn resolved_download_dir(&self) -> PathBuf {
        if let Some(dir) = &self.download_dir {
            if !dir.trim().is_empty() {
                return PathBuf::from(dir);
            }
        }
        // 默认：用户图片目录下的 WallHavenStudio
        #[cfg(windows)]
        {
            if let Ok(profile) = std::env::var("USERPROFILE") {
                return PathBuf::from(profile)
                    .join("Pictures")
                    .join("WallHavenStudio");
            }
            PathBuf::from("WallHavenStudio")
        }
        #[cfg(not(windows))]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home).join("Pictures").join("WallHavenStudio");
            }
            PathBuf::from("WallHavenStudio")
        }
    }
}

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "config path has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("could not create config directory: {error}"))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "config path has no valid file name".to_string())?;
    let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        unique
    ));

    let result = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| format!("could not create temporary config: {error}"))?;
        file.write_all(contents)
            .map_err(|error| format!("could not write temporary config: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("could not flush temporary config: {error}"))?;
        drop(file);

        replace_config_file(&temp_path, path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(windows)]
fn replace_config_file(temp_path: &Path, path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }

    // MoveFileEx is not a reliable atomic replace when the destination already
    // exists. ReplaceFileW replaces it in one filesystem operation.
    if !path.exists() {
        return std::fs::rename(temp_path, path)
            .map_err(|error| format!("could not install config atomically: {error}"));
    }

    let replaced: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let replacement: Vec<u16> = temp_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "could not replace config atomically: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_config_file(temp_path: &Path, path: &Path) -> Result<(), String> {
    std::fs::rename(temp_path, path)
        .map_err(|error| format!("could not replace config atomically: {error}"))
}

/// Windows DPAPI 封装（其他平台明确返回错误，绝不回退为明文）。
mod dpapi {
    /// 加密并 base64 编码。
    #[cfg(windows)]
    pub fn protect_b64(plain: &str) -> Result<String, String> {
        use std::ptr;
        use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
        use windows_sys::Win32::Security::Cryptography::{
            CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        };

        let mut input = plain.as_bytes().to_vec();
        let input_len =
            u32::try_from(input.len()).map_err(|_| "API key is too large for DPAPI".to_string())?;
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: input_len,
            pbData: input.as_mut_ptr(),
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };

        let ok = unsafe {
            CryptProtectData(
                &in_blob,
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        };
        if ok == 0 {
            return Err(format!(
                "DPAPI protection failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let data = if out_blob.cbData == 0 {
            Vec::new()
        } else if out_blob.pbData.is_null() {
            return Err("DPAPI protection returned invalid output".to_string());
        } else {
            unsafe {
                std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec()
            }
        };
        if !out_blob.pbData.is_null() {
            unsafe {
                LocalFree(out_blob.pbData as HLOCAL);
            }
        }
        Ok(b64_encode(&data))
    }

    #[cfg(windows)]
    pub fn unprotect_b64(encrypted: &str) -> Result<String, String> {
        use std::ptr;
        use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
        use windows_sys::Win32::Security::Cryptography::{
            CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        };

        let mut data = b64_decode(encrypted)?;
        let data_len = u32::try_from(data.len())
            .map_err(|_| "protected API key is too large for DPAPI".to_string())?;
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: data_len,
            pbData: data.as_mut_ptr(),
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };

        let ok = unsafe {
            CryptUnprotectData(
                &in_blob,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        };
        if ok == 0 {
            return Err(format!(
                "DPAPI unprotection failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let plain = if out_blob.cbData == 0 {
            Vec::new()
        } else if out_blob.pbData.is_null() {
            return Err("DPAPI unprotection returned invalid output".to_string());
        } else {
            unsafe {
                std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec()
            }
        };
        if !out_blob.pbData.is_null() {
            unsafe {
                LocalFree(out_blob.pbData as HLOCAL);
            }
        }
        String::from_utf8(plain).map_err(|_| "DPAPI output was not valid UTF-8".to_string())
    }

    #[cfg(not(windows))]
    pub fn protect_b64(_plain: &str) -> Result<String, String> {
        Err("DPAPI is only available on Windows".to_string())
    }

    #[cfg(not(windows))]
    pub fn unprotect_b64(_encrypted: &str) -> Result<String, String> {
        Err("DPAPI is only available on Windows".to_string())
    }

    // ---- 最小 base64 实现（避免额外依赖）----
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn b64_encode(input: &[u8]) -> String {
        let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
        for chunk in input.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let value = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[((value >> 18) & 63) as usize] as char);
            out.push(TABLE[((value >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                TABLE[((value >> 6) & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                TABLE[(value & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    pub fn b64_decode(input: &str) -> Result<Vec<u8>, String> {
        let cleaned: Vec<u8> = input
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        if cleaned.is_empty() {
            return Ok(Vec::new());
        }
        if !cleaned.len().is_multiple_of(4) {
            return Err("protected API key is not valid base64".to_string());
        }

        let mut reverse = [u8::MAX; 256];
        for (index, &byte) in TABLE.iter().enumerate() {
            reverse[byte as usize] = index as u8;
        }

        let mut out = Vec::with_capacity(cleaned.len() / 4 * 3);
        let chunk_count = cleaned.len() / 4;
        for (chunk_index, chunk) in cleaned.chunks_exact(4).enumerate() {
            let is_last = chunk_index + 1 == chunk_count;
            let padding = match (chunk[2] == b'=', chunk[3] == b'=') {
                (true, true) => 2,
                (false, true) => 1,
                (false, false) => 0,
                (true, false) => {
                    return Err("protected API key is not valid base64".to_string());
                }
            };
            if (!is_last && padding != 0) || chunk[0] == b'=' || chunk[1] == b'=' {
                return Err("protected API key is not valid base64".to_string());
            }

            let mut values = [0u32; 4];
            for index in 0..(4 - padding) {
                let value = reverse[chunk[index] as usize];
                if value == u8::MAX {
                    return Err("protected API key is not valid base64".to_string());
                }
                values[index] = value as u32;
            }
            let value = (values[0] << 18) | (values[1] << 12) | (values[2] << 6) | values[3];
            out.push((value >> 16) as u8);
            if padding < 2 {
                out.push((value >> 8) as u8);
            }
            if padding == 0 {
                out.push(value as u8);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let unique = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "wh-studio-config-{name}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn config_path(&self) -> PathBuf {
            self.path.join("config.json")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn b64_roundtrip_and_rejects_invalid_data() {
        use dpapi::{b64_decode, b64_encode};
        for value in ["", "a", "ab", "abc", "abcdef", "wallhaven-测试-key"] {
            let encoded = b64_encode(value.as_bytes());
            let decoded = b64_decode(&encoded).unwrap();
            assert_eq!(String::from_utf8(decoded).unwrap(), value);
        }
        assert!(b64_decode("not-base64!").is_err());
        assert!(b64_decode("A==A").is_err());
    }

    #[test]
    fn defaults_have_nsfw_disabled_and_no_key() {
        let settings = Settings::default();
        assert!(!settings.nsfw_enabled);
        assert!(!settings.has_api_key());
        assert!(settings.api_key.is_none());
        assert!(settings.api_key_protected.is_none());
        assert!(settings
            .resolved_download_dir()
            .to_string_lossy()
            .contains("WallHavenStudio"));
    }

    #[test]
    fn parses_utf8_bom_in_isolated_config() {
        let dir = TestDir::new("bom");
        std::fs::write(
            dir.config_path(),
            "\u{feff}{\"download_dir\":\"D:/pics\",\"layout_mode\":1}",
        )
        .unwrap();

        let settings = Settings::load_from_path(&dir.config_path()).unwrap();
        assert_eq!(settings.download_dir.as_deref(), Some("D:/pics"));
        assert_eq!(settings.layout_mode, 1);
        assert!(!settings.has_api_key());
    }

    #[test]
    fn invalid_encrypted_data_is_an_error_without_plaintext_fallback() {
        let dir = TestDir::new("invalid-encrypted");
        let legacy_secret = "must-not-be-used-as-fallback";
        std::fs::write(
            dir.config_path(),
            format!("{{\"api_key_protected\":\"not-base64!\",\"api_key\":\"{legacy_secret}\"}}"),
        )
        .unwrap();

        let error = Settings::load_from_path(&dir.config_path()).unwrap_err();
        assert!(error.contains("could not decrypt API key"));
        assert!(!error.contains(legacy_secret));
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_ciphertext_roundtrip() {
        let secret = "wh-test-dpapi-测试-secret";
        let encrypted = dpapi::protect_b64(secret).unwrap();
        assert_ne!(encrypted, secret);
        assert_eq!(dpapi::unprotect_b64(&encrypted).unwrap(), secret);
    }

    #[cfg(windows)]
    #[test]
    fn serialization_never_contains_plaintext_secret() {
        let dir = TestDir::new("no-plaintext");
        let secret = "WH_TEST_SECRET_DO_NOT_SERIALIZE_7d9c";
        let mut settings = Settings::default();
        settings.set_api_key(Some(secret.to_string())).unwrap();
        settings.download_dir = Some("D:/pics".into());
        settings.save_to_path(&dir.config_path()).unwrap();

        let text = std::fs::read_to_string(dir.config_path()).unwrap();
        assert!(!text.contains(secret));
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(json.get("api_key").is_none());
        assert!(json
            .get("api_key_protected")
            .and_then(|value| value.as_str())
            .is_some());

        let loaded = Settings::load_from_path(&dir.config_path()).unwrap();
        assert_eq!(loaded.api_key.as_deref(), Some(secret));
    }

    #[cfg(windows)]
    #[test]
    fn legacy_plaintext_is_migrated_on_save() {
        let dir = TestDir::new("legacy-migration");
        let secret = "WH_LEGACY_SECRET_MIGRATE_41f2";
        std::fs::write(
            dir.config_path(),
            format!("\u{feff}{{\"api_key\":\"{secret}\",\"nsfw_enabled\":false}}"),
        )
        .unwrap();

        let settings = Settings::load_from_path(&dir.config_path()).unwrap();
        assert_eq!(settings.api_key.as_deref(), Some(secret));
        assert!(settings.api_key_protected.is_none());
        settings.save_to_path(&dir.config_path()).unwrap();

        let migrated = std::fs::read_to_string(dir.config_path()).unwrap();
        assert!(!migrated.contains(secret));
        let json: serde_json::Value = serde_json::from_str(&migrated).unwrap();
        assert!(json.get("api_key").is_none());
        assert!(json.get("api_key_protected").is_some());
        assert_eq!(
            Settings::load_from_path(&dir.config_path())
                .unwrap()
                .api_key
                .as_deref(),
            Some(secret)
        );
    }

    #[cfg(windows)]
    #[test]
    fn clear_key_removes_persisted_ciphertext() {
        let dir = TestDir::new("clear-key");
        let mut settings = Settings::default();
        settings
            .set_api_key(Some("temporary-secret".to_string()))
            .unwrap();
        settings.save_to_path(&dir.config_path()).unwrap();

        settings.set_api_key(None).unwrap();
        assert!(!settings.has_api_key());
        assert!(settings.api_key.is_none());
        assert!(settings.api_key_protected.is_none());
        settings.save_to_path(&dir.config_path()).unwrap();

        let text = std::fs::read_to_string(dir.config_path()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(json.get("api_key").is_none());
        assert!(json.get("api_key_protected").is_none());
        assert!(!Settings::load_from_path(&dir.config_path())
            .unwrap()
            .has_api_key());
    }

    #[test]
    fn account_username_roundtrips_without_credentials() {
        let dir = TestDir::new("account-name");
        let settings = Settings {
            account_username: Some("test-user".into()),
            ..Default::default()
        };
        settings.save_to_path(&dir.config_path()).unwrap();
        let loaded = Settings::load_from_path(&dir.config_path()).unwrap();
        assert_eq!(loaded.account_username.as_deref(), Some("test-user"));
        assert!(!loaded.has_api_key());
        assert_eq!(loaded.layout_mode, 0);
    }
}
