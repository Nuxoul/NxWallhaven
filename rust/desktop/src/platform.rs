//! Small, validated wrappers around desktop platform services.

use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "WallHavenStudio";
const WALLPAPER_DIR_NAME: &str = "Wallpapers";
const WALLPAPER_FILE_NAME: &str = "wallpaper.bmp";
const MAX_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_IMAGE_EDGE: u32 = 20_000;

/// Copies UTF-8 Rust text to the Windows clipboard as `CF_UNICODETEXT`.
///
/// Embedded NUL characters are rejected because the native clipboard format is
/// NUL-terminated. On success, ownership of the allocated memory is transferred
/// to the clipboard; every failure path retains and frees it.
pub fn copy_text(text: &str) -> Result<(), String> {
    if text.contains('\0') {
        return Err("clipboard text contains an embedded NUL character".into());
    }

    #[cfg(windows)]
    {
        windows::copy_text(text)
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        Err("copying text is only supported on Windows".into())
    }
}

/// Opens an existing directory with the system file manager.
pub fn open_folder(path: &Path) -> Result<(), String> {
    validate_directory(path)?;

    #[cfg(windows)]
    {
        windows::open_folder(path)
    }
    #[cfg(not(windows))]
    {
        Err("opening folders is only supported on Windows".into())
    }
}

/// Returns the stable per-user directory used for converted wallpaper files.
pub fn wallpaper_cache_dir() -> PathBuf {
    #[cfg(windows)]
    {
        windows::local_app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(APP_DIR_NAME)
            .join(WALLPAPER_DIR_NAME)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".cache"))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join("wallhaven-studio")
            .join(WALLPAPER_DIR_NAME)
    }
}

/// Validates and decodes an image with bounded resources, converts it to a
/// stable BMP in per-user app data, then asks Windows to set it as wallpaper.
pub fn set_wallpaper(path: &Path) -> Result<(), String> {
    validate_image_path(path)?;

    #[cfg(windows)]
    {
        let cache_dir = windows::local_app_data_dir()?
            .join(APP_DIR_NAME)
            .join(WALLPAPER_DIR_NAME);
        let bmp_path = prepare_wallpaper(path, &cache_dir)?;
        windows::set_wallpaper(&bmp_path)
    }
    #[cfg(not(windows))]
    {
        Err("setting wallpaper is only supported on Windows".into())
    }
}

fn validate_directory(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("folder does not exist or is inaccessible: {error}"))?;
    if !metadata.is_dir() {
        return Err("path is not a directory".into());
    }
    Ok(())
}

fn validate_image_path(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("image does not exist or is inaccessible: {error}"))?;
    if !metadata.is_file() {
        return Err("image path is not a regular file".into());
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "image exceeds the {} MiB source-size limit",
            MAX_SOURCE_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn prepare_wallpaper(source: &Path, cache_dir: &Path) -> Result<PathBuf, String> {
    validate_image_path(source)?;
    let image = decode_bounded_image(source)?;
    std::fs::create_dir_all(cache_dir)
        .map_err(|error| format!("could not create wallpaper cache directory: {error}"))?;

    let destination = cache_dir.join(WALLPAPER_FILE_NAME);
    write_bmp_atomically(&image, &destination)?;
    Ok(destination)
}

#[cfg(any(windows, test))]
fn decode_bounded_image(path: &Path) -> Result<image::DynamicImage, String> {
    use std::io::Read;

    let file = std::fs::File::open(path)
        .map_err(|error| format!("could not open wallpaper image: {error}"))?;
    let reader = std::io::BufReader::new(file).take(MAX_SOURCE_BYTES + 1);
    let mut reader = image::ImageReader::new(reader)
        .with_guessed_format()
        .map_err(|error| format!("could not inspect wallpaper image: {error}"))?;
    if reader.format().is_none() {
        return Err("unsupported or unrecognized wallpaper image format".into());
    }

    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    limits.max_image_width = Some(MAX_IMAGE_EDGE);
    limits.max_image_height = Some(MAX_IMAGE_EDGE);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|error| format!("could not decode wallpaper image within limits: {error}"))
}

#[cfg(any(windows, test))]
fn write_bmp_atomically(image: &image::DynamicImage, destination: &Path) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    let parent = destination
        .parent()
        .ok_or_else(|| "wallpaper destination has no parent directory".to_string())?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "wallpaper destination has no valid file name".to_string())?;
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        unique
    ));

    let result = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("could not create temporary wallpaper: {error}"))?;
        image
            .write_to(&mut file, image::ImageFormat::Bmp)
            .map_err(|error| format!("could not encode wallpaper as BMP: {error}"))?;
        file.flush()
            .map_err(|error| format!("could not flush temporary wallpaper: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("could not sync temporary wallpaper: {error}"))?;
        drop(file);
        replace_file(&temporary, destination)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(all(any(windows, test), windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    windows::replace_file(source, destination)
}

#[cfg(all(any(windows, test), not(windows)))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        std::fs::remove_file(destination)
            .map_err(|error| format!("could not replace cached wallpaper: {error}"))?;
    }
    std::fs::rename(source, destination)
        .map_err(|error| format!("could not install cached wallpaper: {error}"))
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        GetLastError, GlobalFree, SetLastError, ERROR_SUCCESS, HGLOBAL, HWND,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_LocalAppData, SHGetKnownFolderPath, ShellExecuteW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SystemParametersInfoW, HWND_MESSAGE, SPIF_SENDCHANGE,
        SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER, SW_SHOWNORMAL,
    };

    struct ClipboardOwner(HWND);
    impl Drop for ClipboardOwner {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.0);
            }
        }
    }
    struct ClipboardGuard(bool);

    impl ClipboardGuard {
        fn close(mut self) -> Result<(), String> {
            if unsafe { CloseClipboard() } == 0 {
                Err(last_error("could not close clipboard"))
            } else {
                self.0 = false;
                Ok(())
            }
        }
    }

    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CloseClipboard();
                }
            }
        }
    }

    struct GlobalMemory(HGLOBAL);

    impl GlobalMemory {
        fn allocate(bytes: usize) -> Result<Self, String> {
            let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
            if handle.is_null() {
                Err(last_error("could not allocate clipboard memory"))
            } else {
                Ok(Self(handle))
            }
        }

        fn transfer_to_clipboard(mut self) {
            self.0 = null_mut();
        }
    }

    impl Drop for GlobalMemory {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    GlobalFree(self.0);
                }
            }
        }
    }

    struct LockedGlobal(HGLOBAL);

    impl Drop for LockedGlobal {
        fn drop(&mut self) {
            unsafe {
                // A zero return is also the documented success value when the lock count
                // reaches zero. Clear last-error first so there is no ambiguous stale error.
                SetLastError(ERROR_SUCCESS);
                GlobalUnlock(self.0);
            }
        }
    }

    struct CoTaskMemString(*mut u16);

    impl Drop for CoTaskMemString {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CoTaskMemFree(self.0.cast());
                }
            }
        }
    }

    pub(super) fn copy_text(text: &str) -> Result<(), String> {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let bytes = wide
            .len()
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or_else(|| "clipboard text is too large".to_string())?;

        let memory = GlobalMemory::allocate(bytes)?;
        let locked = unsafe { GlobalLock(memory.0) };
        if locked.is_null() {
            return Err(last_error("could not lock clipboard memory"));
        }
        let lock_guard = LockedGlobal(memory.0);
        unsafe {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), locked.cast::<u16>(), wide.len());
        }
        drop(lock_guard);

        let class = wide_str("STATIC");
        let owner = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                null_mut(),
                null(),
            )
        };
        if owner.is_null() {
            return Err(last_error("could not create clipboard owner"));
        }
        let _owner = ClipboardOwner(owner);
        if unsafe { OpenClipboard(owner) } == 0 {
            return Err(last_error("could not open clipboard"));
        }
        let clipboard = ClipboardGuard(true);
        if unsafe { EmptyClipboard() } == 0 {
            return Err(last_error("could not empty clipboard"));
        }
        if unsafe { SetClipboardData(CF_UNICODETEXT as u32, memory.0) }.is_null() {
            return Err(last_error("could not set clipboard data"));
        }
        memory.transfer_to_clipboard();
        clipboard.close()
    }

    pub(super) fn open_folder(path: &Path) -> Result<(), String> {
        let path = wide_path(path)?;
        let operation = wide_str("open");
        let result = unsafe {
            ShellExecuteW(
                null_mut(),
                operation.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            )
        } as isize;
        if result <= 32 {
            Err(format!(
                "could not open folder (ShellExecuteW code {result})"
            ))
        } else {
            Ok(())
        }
    }

    pub(super) fn local_app_data_dir() -> Result<PathBuf, String> {
        let mut raw = null_mut();
        let result =
            unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, null_mut(), &mut raw) };
        if result < 0 {
            return Err(format!(
                "could not find the per-user local app-data directory (HRESULT 0x{:08X})",
                result as u32
            ));
        }
        let owned = CoTaskMemString(raw);
        if owned.0.is_null() {
            return Err("Windows returned an empty local app-data path".into());
        }
        let length = unsafe {
            let mut length = 0usize;
            while *owned.0.add(length) != 0 {
                length += 1;
            }
            length
        };
        let wide = unsafe { std::slice::from_raw_parts(owned.0, length) };
        Ok(PathBuf::from(std::ffi::OsString::from_wide(wide)))
    }

    pub(super) fn set_wallpaper(path: &Path) -> Result<(), String> {
        let mut path = wide_path(path)?;
        let result = unsafe {
            SystemParametersInfoW(
                SPI_SETDESKWALLPAPER,
                0,
                path.as_mut_ptr().cast::<c_void>(),
                SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
            )
        };
        if result == 0 {
            Err(last_error("Windows could not set the desktop wallpaper"))
        } else {
            Ok(())
        }
    }

    pub(super) fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
        let source = wide_path(source)?;
        let destination = wide_path(destination)?;
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(last_error("could not install cached wallpaper"))
        } else {
            Ok(())
        }
    }

    fn wide_str(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path(path: &Path) -> Result<Vec<u16>, String> {
        if path.as_os_str().encode_wide().any(|unit| unit == 0) {
            return Err("path contains an embedded NUL character".into());
        }
        Ok(path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect())
    }

    fn last_error(context: &str) -> String {
        let code = unsafe { GetLastError() };
        if code == ERROR_SUCCESS {
            context.to_string()
        } else {
            format!(
                "{context}: {}",
                std::io::Error::from_raw_os_error(code as i32)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "wh-platform-test-{}-{}",
                std::process::id(),
                unique
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn copy_text_rejects_embedded_nul_without_touching_clipboard() {
        let error = copy_text("left\0right").unwrap_err();
        assert!(error.contains("NUL"));
    }

    #[test]
    fn directory_validation_rejects_missing_paths_and_files() {
        let temp = TestDir::new();
        let file = temp.0.join("file.txt");
        std::fs::write(&file, b"not a directory").unwrap();

        assert!(open_folder(&temp.0.join("missing")).is_err());
        assert!(open_folder(&file).is_err());
        assert!(validate_directory(&temp.0).is_ok());
        // Do not call open_folder with the valid directory: tests must not launch Explorer.
    }

    #[test]
    fn wallpaper_validation_rejects_missing_paths_and_directories() {
        let temp = TestDir::new();
        assert!(set_wallpaper(&temp.0.join("missing.png")).is_err());
        assert!(set_wallpaper(&temp.0).is_err());
    }

    #[test]
    fn invalid_image_is_rejected_before_any_platform_call() {
        let temp = TestDir::new();
        let source = temp.0.join("bad.img");
        std::fs::write(&source, b"not an image").unwrap();
        let cache = temp.0.join("cache");

        assert!(prepare_wallpaper(&source, &cache).is_err());
        assert!(!cache.exists());
    }

    #[test]
    fn oversized_source_is_rejected_without_decoding() {
        let temp = TestDir::new();
        let source = temp.0.join("oversized.img");
        let file = std::fs::File::create(&source).unwrap();
        file.set_len(MAX_SOURCE_BYTES + 1).unwrap();
        drop(file);

        let cache = temp.0.join("cache");
        let error = prepare_wallpaper(&source, &cache).unwrap_err();
        assert!(error.contains("source-size limit"), "{error}");
        assert!(!cache.exists());
    }

    #[test]
    fn dimensions_over_the_decoder_limit_are_rejected() {
        let temp = TestDir::new();
        let source = temp.0.join("too-wide.png");
        // A complete PNG signature/IHDR is sufficient for the decoder to apply
        // the strict dimensions limit before allocating or reading pixel data.
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&(MAX_IMAGE_EDGE + 1).to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend_from_slice(&0u32.to_be_bytes()); // CRC is not reached before limit checking.
        std::fs::write(&source, png).unwrap();

        let error = decode_bounded_image(&source).unwrap_err();
        assert!(
            error.contains("limit") || error.contains("Limits"),
            "{error}"
        );
    }

    #[test]
    fn image_is_converted_to_stable_bmp_path() {
        let temp = TestDir::new();
        let source = temp.0.join("壁纸 图像.png");
        let source_image = image::RgbaImage::from_fn(2, 2, |x, y| {
            image::Rgba([(x * 80) as u8, (y * 80) as u8, 120, 255])
        });
        source_image
            .save_with_format(&source, image::ImageFormat::Png)
            .unwrap();

        let cache = temp.0.join("壁纸缓存");
        let destination = prepare_wallpaper(&source, &cache).unwrap();
        assert_eq!(destination, cache.join(WALLPAPER_FILE_NAME));
        let bytes = std::fs::read(&destination).unwrap();
        assert_eq!(&bytes[..2], b"BM");
        let decoded = image::open(destination).unwrap();
        assert_eq!(decoded.dimensions(), (2, 2));
    }

    #[test]
    fn repeated_conversion_replaces_the_same_cache_file() {
        let temp = TestDir::new();
        let source = temp.0.join("source.png");
        let cache = temp.0.join("cache");

        image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]))
            .save(&source)
            .unwrap();
        let first = prepare_wallpaper(&source, &cache).unwrap();

        image::RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 255]))
            .save(&source)
            .unwrap();
        let second = prepare_wallpaper(&source, &cache).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            image::open(second).unwrap().to_rgb8().get_pixel(0, 0).0,
            [0, 0, 255]
        );
    }
}
