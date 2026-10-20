//! Constant-memory container validation; never decode an image into RGBA.
//! Network framing/length and SHA-256 are checked separately by the cache.
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

type Result<T> = io::Result<T>;
fn bad() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid image container")
}
fn byte(r: &mut impl Read) -> Result<u8> {
    let mut b = [0];
    r.read_exact(&mut b)?;
    Ok(b[0])
}
fn skip(r: &mut impl Read, n: u64) -> Result<()> {
    if io::copy(&mut r.take(n), &mut io::sink())? != n {
        return Err(bad());
    }
    Ok(())
}
fn end(r: &mut impl Read) -> Result<()> {
    if r.read(&mut [0])? == 0 {
        Ok(())
    } else {
        Err(bad())
    }
}

pub(crate) fn validate(path: &Path) -> Result<()> {
    let mut r = BufReader::new(File::open(path)?);
    let mut magic = [0; 12];
    let n = r.read(&mut magic)?;
    r.seek(SeekFrom::Start(0))?;
    if n >= 8 && magic[..8] == *b"\x89PNG\r\n\x1a\n" {
        png(&mut r)
    } else if n >= 2 && magic[..2] == [0xff, 0xd8] {
        jpeg(&mut r)
    } else if n >= 6 && (&magic[..6] == b"GIF87a" || &magic[..6] == b"GIF89a") {
        gif(&mut r)
    } else if n == 12 && &magic[..4] == b"RIFF" && &magic[8..] == b"WEBP" {
        webp(&mut r)
    } else {
        Err(bad())
    }
}
fn png(r: &mut impl Read) -> Result<()> {
    skip(r, 8)?;
    let mut first = true;
    let mut data = false;
    loop {
        let mut header = [0; 8];
        r.read_exact(&mut header)?;
        let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as u64;
        let kind = &header[4..];
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err(bad());
        }
        if first && (kind != b"IHDR" || len != 13) {
            return Err(bad());
        }
        if !first && kind == b"IHDR" {
            return Err(bad());
        }
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        let mut left = len;
        let mut buf = [0; 32 * 1024];
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            r.read_exact(&mut buf[..n])?;
            if first {
                let w = u32::from_be_bytes(buf[..4].try_into().unwrap());
                let h = u32::from_be_bytes(buf[4..8].try_into().unwrap());
                let depth = buf[8];
                let color = buf[9];
                let valid_depth = match color {
                    0 => [1, 2, 4, 8, 16].contains(&depth),
                    2 | 4 | 6 => [8, 16].contains(&depth),
                    3 => [1, 2, 4, 8].contains(&depth),
                    _ => false,
                };
                if w == 0 || h == 0 || !valid_depth || buf[10] != 0 || buf[11] != 0 || buf[12] > 1 {
                    return Err(bad());
                }
            }
            crc.update(&buf[..n]);
            left -= n as u64;
        }
        let mut expected = [0; 4];
        r.read_exact(&mut expected)?;
        if crc.finalize() != u32::from_be_bytes(expected) {
            return Err(bad());
        }
        if kind == b"IDAT" && len > 0 {
            data = true;
        }
        if kind == b"IEND" {
            if len != 0 || !data {
                return Err(bad());
            }
            return end(r);
        }
        first = false;
    }
}
fn jpeg(r: &mut impl BufRead) -> Result<()> {
    skip(r, 2)?;
    let mut frame = false;
    let mut scan = false;
    let mut pending = None;
    loop {
        let marker = if let Some(m) = pending.take() {
            m
        } else {
            if byte(r)? != 0xff {
                return Err(bad());
            }
            let mut m = byte(r)?;
            while m == 0xff {
                m = byte(r)?;
            }
            m
        };
        if marker == 0xd9 {
            if !frame || !scan {
                return Err(bad());
            }
            return end(r);
        }
        if marker == 0 || marker == 0xd8 || (0xd0..=0xd7).contains(&marker) {
            return Err(bad());
        }
        let mut length = [0; 2];
        r.read_exact(&mut length)?;
        let length = u16::from_be_bytes(length);
        if length < 2 {
            return Err(bad());
        }
        let mut left = length as u64 - 2;
        if [0xc0, 0xc1, 0xc2].contains(&marker) {
            if left < 6 {
                return Err(bad());
            }
            let mut h = [0; 6];
            r.read_exact(&mut h)?;
            left -= 6;
            if h[0] != 8
                || h[1..3] == [0, 0]
                || h[3..5] == [0, 0]
                || h[5] == 0
                || left != 3 * h[5] as u64
            {
                return Err(bad());
            }
            frame = true;
        }
        if marker == 0xda {
            if !frame || left < 6 {
                return Err(bad());
            }
            scan = true;
        }
        skip(r, left)?;
        if marker == 0xda {
            // Scan entropy without decoding. Stuffed FF and restart markers stay in scan.
            loop {
                let buf = r.fill_buf()?;
                if buf.is_empty() {
                    return Err(bad());
                }
                let n = buf.iter().position(|b| *b == 0xff).unwrap_or(buf.len());
                r.consume(n);
                if n == 0 {
                    byte(r)?;
                    let mut m = byte(r)?;
                    while m == 0xff {
                        m = byte(r)?;
                    }
                    if m == 0 || (0xd0..=0xd7).contains(&m) {
                        continue;
                    }
                    pending = Some(m);
                    break;
                }
            }
        }
    }
}
fn subblocks(r: &mut impl Read) -> Result<()> {
    loop {
        let n = byte(r)?;
        if n == 0 {
            return Ok(());
        }
        skip(r, n as u64)?;
    }
}
fn gif(r: &mut impl Read) -> Result<()> {
    skip(r, 6)?;
    let mut screen = [0; 7];
    r.read_exact(&mut screen)?;
    if screen[..2] == [0, 0] || screen[2..4] == [0, 0] {
        return Err(bad());
    }
    if screen[4] & 0x80 != 0 {
        skip(r, 3 * (1u64 << ((screen[4] & 7) + 1)))?;
    }
    let mut image = false;
    loop {
        match byte(r)? {
            0x3b if image => return end(r),
            0x21 => {
                byte(r)?;
                subblocks(r)?;
            }
            0x2c => {
                let mut d = [0; 9];
                r.read_exact(&mut d)?;
                if d[4..6] == [0, 0] || d[6..8] == [0, 0] {
                    return Err(bad());
                }
                if d[8] & 0x80 != 0 {
                    skip(r, 3 * (1u64 << ((d[8] & 7) + 1)))?;
                }
                if !(2..=8).contains(&byte(r)?) {
                    return Err(bad());
                }
                subblocks(r)?;
                image = true;
            }
            _ => return Err(bad()),
        }
    }
}
fn webp(r: &mut impl Read) -> Result<()> {
    let mut header = [0; 12];
    r.read_exact(&mut header)?;
    let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as u64;
    if size < 4 {
        return Err(bad());
    }
    let mut left = size - 4;
    let mut image = false;
    while left > 0 {
        if left < 8 {
            return Err(bad());
        }
        let mut h = [0; 8];
        r.read_exact(&mut h)?;
        let size = u32::from_le_bytes(h[4..].try_into().unwrap()) as u64;
        let padded = size + size % 2;
        if padded > left - 8 {
            return Err(bad());
        }
        if &h[..4] == b"VP8 " || &h[..4] == b"VP8L" {
            if size < 5 {
                return Err(bad());
            }
            image = true;
        }
        skip(r, padded)?;
        left -= 8 + padded;
    }
    if !image {
        return Err(bad());
    }
    end(r)
}

#[cfg(test)]
pub(crate) fn fixture() -> Vec<u8> {
    // A valid 1x1 PNG, stored compressed exactly as an HTTP original.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [
        (b"IHDR", vec![0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]),
        (b"IDAT", vec![120, 156, 99, 96, 0, 2, 0, 0, 5, 0, 1]),
        (b"IEND", vec![]),
    ] {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(&data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(&data);
        png.extend_from_slice(&crc.finalize().to_be_bytes());
    }
    png
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_checks_structure_crc_and_terminator_without_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a");
        let png = fixture();
        std::fs::write(&path, &png).unwrap();
        assert!(validate(&path).is_ok());
        for n in [0, 8, 20, png.len() - 1] {
            std::fs::write(&path, &png[..n]).unwrap();
            assert!(validate(&path).is_err());
        }
        let mut corrupt = png.clone();
        corrupt[45] ^= 1;
        std::fs::write(&path, corrupt).unwrap();
        assert!(validate(&path).is_err());
        std::fs::write(&path, b"<html>not an image</html>").unwrap();
        assert!(validate(&path).is_err());
    }
}
