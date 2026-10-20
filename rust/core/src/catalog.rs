//! Local catalog, independent of six-character wallpaper/download IDs.
//!
//! Scans are read-only and limited to 10,000 entries / 32 directory levels; exceeding
//! either limit is an error, never a silently truncated preview. Image bytes are
//! streamed, not decoded to pixels, and have no file-size or image-edge limit.
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 10_000;
const MAX_DEPTH: usize = 32;
type Result<T> = std::result::Result<T, String>;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportCandidate {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub tags: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreview {
    pub files: Vec<ImportCandidate>,
    pub skipped: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogItem {
    pub id: String,
    /// First currently matching source, or the first recorded path if unavailable.
    pub path: String,
    pub paths: Vec<String>,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub tags: Vec<String>,
    /// Unix milliseconds, matching the existing library's timestamps.
    pub created: u64,
    /// At least one regular, non-link source still has this exact digest.
    pub available: bool,
}
pub struct Catalog {
    connection: Mutex<Connection>,
}

pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|tag| {
            tag.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|tag| !tag.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

impl Catalog {
    pub fn open(data_dir: &Path) -> Result<Self> {
        fs::create_dir_all(data_dir).map_err(error)?;
        let mut connection =
            Connection::open(data_dir.join("library-v1.sqlite3")).map_err(error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(error)?;
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(error)?;
        if version > 1 {
            return Err("unsupported library database version".into());
        }
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(error)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(error)?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS catalog_items (
                id TEXT PRIMARY KEY NOT NULL, bytes INTEGER NOT NULL,
                width INTEGER NOT NULL, height INTEGER NOT NULL, mime TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '[]', created INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS catalog_sources (
                id TEXT NOT NULL REFERENCES catalog_items(id) ON DELETE CASCADE,
                path TEXT NOT NULL, PRIMARY KEY(id,path));
             PRAGMA user_version=1;",
        )
        .map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn db(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| "catalog lock poisoned".into())
    }

    pub fn scan(root: &Path, recursive: bool) -> Result<ImportPreview> {
        let root = safe_path(root)?;
        let meta = fs::symlink_metadata(&root).map_err(error)?;
        let mut preview = ImportPreview {
            files: Vec::new(),
            skipped: 0,
        };
        if meta.is_file() {
            match inspect(&root) {
                Ok((candidate, _)) => preview.files.push(candidate),
                Err(_) => preview.skipped += 1,
            }
            return Ok(preview);
        }
        if !meta.is_dir() {
            return Err("scan root must be a regular file or directory".into());
        }
        let mut pending = vec![(root.clone(), 0)];
        let mut seen = 0;
        while let Some((dir, depth)) = pending.pop() {
            let checked = safe_path(&dir)?;
            if !checked.starts_with(&root) {
                return Err("scan path escaped root".into());
            }
            for entry in fs::read_dir(&checked).map_err(error)? {
                seen += 1;
                if seen > MAX_ENTRIES {
                    return Err("scan exceeds 10000 entries".into());
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => {
                        preview.skipped += 1;
                        continue;
                    }
                };
                let path = entry.path();
                let meta = match fs::symlink_metadata(&path) {
                    Ok(m) => m,
                    Err(_) => {
                        preview.skipped += 1;
                        continue;
                    }
                };
                if is_link(&meta) {
                    preview.skipped += 1;
                    continue;
                }
                if meta.is_dir() && recursive {
                    if depth >= MAX_DEPTH {
                        return Err("scan exceeds 32 directory levels".into());
                    }
                    pending.push((path, depth + 1));
                } else if meta.is_file() {
                    match inspect(&path) {
                        Ok((candidate, _)) if Path::new(&candidate.path).starts_with(&root) => {
                            preview.files.push(candidate)
                        }
                        _ => preview.skipped += 1,
                    }
                } else {
                    preview.skipped += 1;
                }
            }
        }
        preview.files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(preview)
    }

    /// Returns the number of newly inserted content IDs, not source paths.
    /// All candidates are re-read before a transaction; any mismatch rejects the
    /// entire batch. Kept read handles also deny concurrent writes/deletes on Windows.
    pub fn import(&self, preview: &ImportPreview) -> Result<usize> {
        if preview.files.len() > MAX_ENTRIES {
            return Err("import exceeds 10000 entries".into());
        }
        let mut verified = Vec::with_capacity(preview.files.len());
        for candidate in &preview.files {
            let (actual, file) = inspect(Path::new(&candidate.path))?;
            if actual.path != candidate.path
                || actual.sha256 != candidate.sha256
                || actual.bytes != candidate.bytes
                || actual.width != candidate.width
                || actual.height != candidate.height
                || actual.mime != candidate.mime
            {
                return Err(format!("file changed since preview: {}", candidate.path));
            }
            verified.push((actual, file));
        }
        let mut db = self.db()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let mut inserted = 0;
        for ((actual, _file), candidate) in verified.iter().zip(&preview.files) {
            inserted += tx.execute(
                "INSERT OR IGNORE INTO catalog_items (id,bytes,width,height,mime,tags,created) VALUES (?1,?2,?3,?4,?5,'[]',?6)",
                params![actual.sha256, i64::try_from(actual.bytes).map_err(error)?, actual.width, actual.height, actual.mime, created]
            ).map_err(error)?;
            let old: String = tx
                .query_row(
                    "SELECT tags FROM catalog_items WHERE id=?1",
                    [&actual.sha256],
                    |r| r.get(0),
                )
                .map_err(error)?;
            let mut tags: Vec<String> = serde_json::from_str(&old).map_err(error)?;
            tags.extend(candidate.tags.iter().cloned());
            tx.execute(
                "UPDATE catalog_items SET tags=?2 WHERE id=?1",
                params![
                    actual.sha256,
                    serde_json::to_string(&normalize_tags(&tags)).map_err(error)?
                ],
            )
            .map_err(error)?;
            tx.execute(
                "INSERT OR IGNORE INTO catalog_sources (id,path) VALUES (?1,?2)",
                params![actual.sha256, actual.path],
            )
            .map_err(error)?;
        }
        // Check again immediately before commit, including paths/ancestors. This
        // also catches ordinary concurrent edits on platforms without share locks.
        for (actual, file) in &mut verified {
            if safe_path(Path::new(&actual.path))?.to_str() != Some(actual.path.as_str())
                || digest(file, actual.bytes)? != actual.sha256
                || !source_matches(&actual.path, &actual.sha256, actual.bytes)
            {
                return Err("source changed during import".into());
            }
        }
        tx.commit().map_err(error)?;
        Ok(inserted)
    }

    pub fn items(&self) -> Result<Vec<CatalogItem>> {
        let mut items = {
            let db = self.db()?;
            let mut stmt = db.prepare("SELECT id,bytes,width,height,mime,tags,created FROM catalog_items ORDER BY created,id").map_err(error)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, u32>(2)?,
                        r.get::<_, u32>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                })
                .map_err(error)?;
            let mut result = Vec::new();
            for row in rows {
                let (id, bytes, width, height, mime, tags, created) = row.map_err(error)?;
                let bytes = u64::try_from(bytes).map_err(error)?;
                let created = u64::try_from(created).map_err(error)?;
                let mut sources = db
                    .prepare("SELECT path FROM catalog_sources WHERE id=?1 ORDER BY path")
                    .map_err(error)?;
                let paths = sources
                    .query_map([&id], |r| r.get::<_, String>(0))
                    .map_err(error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(error)?;
                result.push(CatalogItem {
                    id,
                    path: paths.first().cloned().unwrap_or_default(),
                    paths,
                    bytes,
                    width,
                    height,
                    mime,
                    tags: normalize_tags(
                        &serde_json::from_str::<Vec<String>>(&tags).map_err(error)?,
                    ),
                    created,
                    available: false,
                });
            }
            result
        };
        for item in &mut items {
            if let Some(path) = item
                .paths
                .iter()
                .find(|path| source_matches(path, &item.id, item.bytes))
            {
                item.path.clone_from(path);
                item.available = true;
            }
        }
        Ok(items)
    }

    /// Removes database records only; never deletes, moves or rewrites a source.
    pub fn remove(&self, id: &str) -> Result<()> {
        valid_id(id)?;
        self.db()?
            .execute("DELETE FROM catalog_items WHERE id=?1", [id])
            .map_err(error)?;
        Ok(())
    }
    pub fn set_tags(&self, id: &str, tags: Vec<String>) -> Result<()> {
        valid_id(id)?;
        let n = self
            .db()?
            .execute(
                "UPDATE catalog_items SET tags=?2 WHERE id=?1",
                params![
                    id,
                    serde_json::to_string(&normalize_tags(&tags)).map_err(error)?
                ],
            )
            .map_err(error)?;
        if n == 0 {
            return Err("catalog item not found".into());
        }
        Ok(())
    }
    /// Current matching source paths only: stale/missing/changed paths aren't duplicates.
    pub fn duplicate_groups(&self) -> Result<Vec<Vec<String>>> {
        let mut groups = Vec::new();
        for item in self.items()? {
            let paths: Vec<_> = item
                .paths
                .into_iter()
                .filter(|p| source_matches(p, &item.id, item.bytes))
                .collect();
            if paths.len() > 1 {
                groups.push(paths);
            }
        }
        Ok(groups)
    }
    /// Only typed local catalog fields are exported, never download/network settings.
    pub fn export_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.items()?).map_err(error)
    }
}

fn valid_id(id: &str) -> Result<()> {
    if id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err("invalid SHA256 catalog ID".into())
    }
}
fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Includes junctions and other reparse points, not just symbolic links.
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}
/// Check every ancestor before canonicalization so a linked root/ancestor cannot
/// be hidden by resolving it first. Parent components are intentionally rejected.
fn safe_path(path: &Path) -> Result<PathBuf> {
    if path.components().any(|c| c == Component::ParentDir) {
        return Err("parent path components are not allowed".into());
    }
    #[cfg(windows)]
    if path.components().any(|c| matches!(c, Component::Prefix(p) if matches!(p.kind(), std::path::Prefix::DeviceNS(_) | std::path::Prefix::Verbatim(_)))) {
        return Err("Windows device paths are not allowed".into());
    }
    let absolute = std::path::absolute(path).map_err(error)?;
    let mut current = PathBuf::new();
    for component in absolute.components() {
        if component == Component::ParentDir {
            return Err("parent path components are not allowed".into());
        }
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let meta = fs::symlink_metadata(&current).map_err(error)?;
        if is_link(&meta) {
            return Err("symlinks and reparse points are not allowed".into());
        }
    }
    let canonical = fs::canonicalize(&absolute).map_err(error)?;
    if canonical.to_str().is_none() {
        return Err("source path is not UTF-8".into());
    }
    Ok(canonical)
}
fn open_source(path: &Path) -> Result<(PathBuf, File)> {
    let path = safe_path(path)?;
    let meta = fs::symlink_metadata(&path).map_err(error)?;
    if !meta.is_file() || is_link(&meta) {
        return Err("source is not a regular non-link file".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000).share_mode(1); // OPEN_REPARSE_POINT, FILE_SHARE_READ
    }
    let file = options.open(&path).map_err(error)?;
    let opened = file.metadata().map_err(error)?;
    if !opened.is_file() || is_link(&opened) || safe_path(&path)? != path {
        return Err("source changed while opening".into());
    }
    Ok((path, file))
}
fn digest(file: &mut File, bytes: u64) -> Result<String> {
    file.seek(SeekFrom::Start(0)).map_err(error)?;
    let mut hasher = Sha256::new();
    let mut remaining = bytes;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let size = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..size]).map_err(error)?;
        hasher.update(&buffer[..size]);
        remaining -= size as u64;
    }
    if file.read(&mut buffer[..1]).map_err(error)? != 0 {
        return Err("source length changed".into());
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn inspect(path: &Path) -> Result<(ImportCandidate, File)> {
    let (path, mut file) = open_source(path)?;
    let before = file.metadata().map_err(error)?;
    let bytes = before.len();
    let (width, height, mime) = dimensions(&mut file, bytes)?;
    let sha256 = digest(&mut file, bytes)?;
    let after = file.metadata().map_err(error)?;
    if after.len() != bytes
        || before.modified().ok() != after.modified().ok()
        || safe_path(&path)? != path
    {
        return Err("source changed while scanning".into());
    }
    Ok((
        ImportCandidate {
            path: path.to_str().ok_or("source path is not UTF-8")?.to_owned(),
            sha256,
            bytes,
            width,
            height,
            mime: mime.into(),
            tags: Vec::new(),
        },
        file,
    ))
}
fn source_matches(path: &str, id: &str, bytes: u64) -> bool {
    open_source(Path::new(path))
        .and_then(|(_, mut file)| {
            if file.metadata().map_err(error)?.len() != bytes {
                return Ok(false);
            }
            Ok(digest(&mut file, bytes)? == id)
        })
        .unwrap_or(false)
}

// Structural, seek-based readers: fixed-size buffers, no pixel allocation, no
// encoded-file allocation. Payloads are bounds-checked and skipped, then the
// complete original file is streamed through SHA256. This is header/container
// validation, not a guarantee that every compressed pixel can be decoded.
fn read<const N: usize>(file: &mut File) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    file.read_exact(&mut bytes).map_err(error)?;
    Ok(bytes)
}
fn skip(file: &mut File, count: u64, end: u64) -> Result<()> {
    let pos = file.stream_position().map_err(error)?;
    let next = pos
        .checked_add(count)
        .filter(|next| *next <= end)
        .ok_or("truncated image structure")?;
    file.seek(SeekFrom::Start(next)).map_err(error)?;
    Ok(())
}
fn dimensions(file: &mut File, bytes: u64) -> Result<(u32, u32, &'static str)> {
    let header = read::<12>(file)?;
    file.seek(SeekFrom::Start(0)).map_err(error)?;
    let (width, height, mime) = if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        let (w, h) = png_dimensions(file, bytes)?;
        (w, h, "image/png")
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        let (w, h) = gif_dimensions(file, bytes)?;
        (w, h, "image/gif")
    } else if header.starts_with(b"RIFF") && &header[8..12] == b"WEBP" {
        let (w, h) = webp_dimensions(file, bytes)?;
        (w, h, "image/webp")
    } else if header.starts_with(&[0xff, 0xd8]) {
        let (w, h) = jpeg_dimensions(file, bytes)?;
        (w, h, "image/jpeg")
    } else {
        return Err("unsupported image format".into());
    };
    if width == 0 || height == 0 {
        return Err("zero image dimensions".into());
    }
    Ok((width, height, mime))
}
fn png_dimensions(file: &mut File, end: u64) -> Result<(u32, u32)> {
    skip(file, 8, end)?;
    let mut dims = None;
    let mut data = false;
    loop {
        let length = u32::from_be_bytes(read(file)?) as u64;
        let kind = read::<4>(file)?;
        if dims.is_none() {
            if &kind != b"IHDR" || length != 13 {
                return Err("invalid PNG header".into());
            }
            let header = read::<13>(file)?;
            let depth = header[8];
            let valid_depth = match header[9] {
                0 => [1, 2, 4, 8, 16].contains(&depth),
                2 | 4 | 6 => [8, 16].contains(&depth),
                3 => [1, 2, 4, 8].contains(&depth),
                _ => false,
            };
            if !valid_depth || header[10] != 0 || header[11] != 0 || header[12] > 1 {
                return Err("invalid PNG header".into());
            }
            dims = Some((
                u32::from_be_bytes(header[0..4].try_into().map_err(error)?),
                u32::from_be_bytes(header[4..8].try_into().map_err(error)?),
            ));
        } else {
            if &kind == b"IHDR" {
                return Err("duplicate PNG header".into());
            }
            skip(file, length, end)?;
        }
        skip(file, 4, end)?; // CRC; pixel/CRC validation is deliberately not full decoding.
        if &kind == b"IDAT" {
            data |= length > 0;
        }
        if &kind == b"IEND" {
            if length != 0 || !data {
                return Err("PNG missing image data".into());
            }
            return dims.ok_or("PNG missing dimensions".into());
        }
    }
}
fn gif_blocks(file: &mut File, end: u64) -> Result<bool> {
    let mut data = false;
    loop {
        let size = read::<1>(file)?[0];
        if size == 0 {
            return Ok(data);
        }
        data = true;
        skip(file, size.into(), end)?;
    }
}
fn gif_dimensions(file: &mut File, end: u64) -> Result<(u32, u32)> {
    let header = read::<13>(file)?;
    let dims = (
        u16::from_le_bytes([header[6], header[7]]) as u32,
        u16::from_le_bytes([header[8], header[9]]) as u32,
    );
    if header[10] & 0x80 != 0 {
        skip(file, 3 * (1 << ((header[10] & 7) + 1)), end)?;
    }
    let mut frame = false;
    loop {
        match read::<1>(file)?[0] {
            0x21 => {
                read::<1>(file)?;
                gif_blocks(file, end)?;
            }
            0x2c => {
                let desc = read::<9>(file)?;
                if u16::from_le_bytes([desc[4], desc[5]]) == 0
                    || u16::from_le_bytes([desc[6], desc[7]]) == 0
                {
                    return Err("invalid GIF frame".into());
                }
                if desc[8] & 0x80 != 0 {
                    skip(file, 3 * (1 << ((desc[8] & 7) + 1)), end)?;
                }
                if !(2..=8).contains(&read::<1>(file)?[0]) || !gif_blocks(file, end)? {
                    return Err("invalid GIF image data".into());
                }
                frame = true;
            }
            0x3b if frame => return Ok(dims),
            _ => return Err("invalid GIF structure".into()),
        }
    }
}
fn webp_dimensions(file: &mut File, bytes: u64) -> Result<(u32, u32)> {
    let header = read::<12>(file)?;
    let end = u32::from_le_bytes(header[4..8].try_into().map_err(error)?) as u64 + 8;
    if end > bytes || end < 20 {
        return Err("invalid WebP container length".into());
    }
    let mut dims = None;
    let mut data = false;
    while file.stream_position().map_err(error)? < end {
        let kind = read::<4>(file)?;
        let length = u32::from_le_bytes(read(file)?) as u64;
        let start = file.stream_position().map_err(error)?;
        let next = start
            .checked_add(length + (length & 1))
            .filter(|n| *n <= end)
            .ok_or("truncated WebP chunk")?;
        match &kind {
            b"VP8X" if length == 10 => {
                let h = read::<10>(file)?;
                dims = Some((
                    1 + u32::from_le_bytes([h[4], h[5], h[6], 0]),
                    1 + u32::from_le_bytes([h[7], h[8], h[9], 0]),
                ));
            }
            b"VP8 " if length >= 10 => {
                let h = read::<10>(file)?;
                if h[0] & 1 != 0 || h[3..6] != [0x9d, 1, 0x2a] {
                    return Err("invalid WebP VP8 header".into());
                }
                let frame = (
                    (u16::from_le_bytes([h[6], h[7]]) & 0x3fff) as u32,
                    (u16::from_le_bytes([h[8], h[9]]) & 0x3fff) as u32,
                );
                if dims.is_none() {
                    dims = Some(frame);
                }
                data = true;
            }
            b"VP8L" if length >= 5 => {
                let h = read::<5>(file)?;
                if h[0] != 0x2f || h[4] >> 5 != 0 {
                    return Err("invalid WebP lossless header".into());
                }
                let bits = u32::from_le_bytes(h[1..5].try_into().map_err(error)?);
                if dims.is_none() {
                    dims = Some((1 + (bits & 0x3fff), 1 + ((bits >> 14) & 0x3fff)));
                }
                data = true;
            }
            b"ANMF" if length >= 24 && dims.is_some() => {
                data = true;
            }
            _ => {}
        }
        file.seek(SeekFrom::Start(next)).map_err(error)?;
    }
    if !data {
        return Err("WebP missing frame data".into());
    }
    dims.ok_or("WebP missing dimensions".into())
}
fn jpeg_dimensions(file: &mut File, end: u64) -> Result<(u32, u32)> {
    skip(file, 2, end)?;
    let mut dims = None;
    loop {
        if read::<1>(file)?[0] != 0xff {
            return Err("invalid JPEG marker".into());
        }
        let mut marker = read::<1>(file)?[0];
        while marker == 0xff {
            marker = read::<1>(file)?[0];
        }
        if matches!(marker, 0x00 | 0xd8 | 0xd9) {
            return Err("JPEG missing scan data".into());
        }
        if matches!(marker, 0x01 | 0xd0..=0xd7) {
            continue;
        }
        let length = u16::from_be_bytes(read(file)?) as u64;
        if length < 2 {
            return Err("invalid JPEG segment length".into());
        }
        let start = file.stream_position().map_err(error)?;
        let next = start
            .checked_add(length - 2)
            .filter(|n| *n <= end)
            .ok_or("truncated JPEG segment")?;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if length < 8 {
                return Err("invalid JPEG frame".into());
            }
            let h = read::<6>(file)?;
            if h[0] == 0 || h[5] == 0 || length != 8 + 3 * u64::from(h[5]) {
                return Err("invalid JPEG frame".into());
            }
            dims = Some((
                u16::from_be_bytes([h[3], h[4]]) as u32,
                u16::from_be_bytes([h[1], h[2]]) as u32,
            ));
        }
        if marker == 0xda {
            let components = read::<1>(file)?[0];
            if components == 0 || length != 6 + 2 * u64::from(components) || next + 2 >= end {
                return Err("invalid JPEG scan".into());
            }
            // Find EOI with fixed storage, allowing preserved trailing metadata.
            // This is structural validation, not DCT/pixel decoding.
            file.seek(SeekFrom::Start(next)).map_err(error)?;
            let mut remaining = end - next;
            let mut buffer = [0u8; 64 * 1024];
            let mut ff = false;
            let mut entropy = false;
            while remaining > 0 {
                let size = remaining.min(buffer.len() as u64) as usize;
                file.read_exact(&mut buffer[..size]).map_err(error)?;
                remaining -= size as u64;
                for &byte in &buffer[..size] {
                    if ff && byte == 0xd9 {
                        if !entropy {
                            return Err("JPEG missing entropy data".into());
                        }
                        return dims.ok_or("JPEG missing dimensions".into());
                    }
                    if byte != 0xff {
                        entropy = true;
                    }
                    ff = byte == 0xff;
                }
            }
            return Err("JPEG missing end marker".into());
        }
        file.seek(SeekFrom::Start(next)).map_err(error)?;
    }
}

#[cfg(test)]
mod tests;
