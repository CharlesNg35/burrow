use std::io::Read;

const MAX_BINARY: u64 = 256 * 1024 * 1024;

fn base_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

fn octal(field: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(field).ok()?;
    let text = text.trim_matches(|c: char| c == '\0' || c == ' ');
    if text.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(text, 8).ok()
}

pub fn from_tar_gz(data: &[u8], want: &str) -> Result<Vec<u8>, String> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(MAX_BINARY)
        .read_to_end(&mut tar)
        .map_err(|e| format!("the archive is not valid gzip: {e}"))?;
    let mut at = 0;
    while at + 512 <= tar.len() {
        let header = &tar[at..at + 512];
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let name_end = header[..100].iter().position(|&b| b == 0).unwrap_or(100);
        let name = String::from_utf8_lossy(&header[..name_end]);
        let size = octal(&header[124..136]).ok_or("the archive has a corrupt tar header")?;
        let kind = header[156];
        let start = at + 512;
        let end = usize::try_from(size)
            .ok()
            .and_then(|s| start.checked_add(s))
            .filter(|&e| e <= tar.len())
            .ok_or("the archive is truncated")?;
        if (kind == b'0' || kind == 0) && base_name(&name) == want {
            return Ok(tar[start..end].to_vec());
        }
        at = start + (end - start).div_ceil(512) * 512;
    }
    Err(format!("the archive does not contain {want}"))
}

// Offsets come from the archive, so every sum is checked: on a 32-bit
// target a u32 offset plus a length can pass usize::MAX.
fn u16_at(d: &[u8], at: usize) -> Option<usize> {
    d.get(at..at.checked_add(2)?)
        .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
}

fn u32_at(d: &[u8], at: usize) -> Option<usize> {
    d.get(at..at.checked_add(4)?)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

pub fn from_zip(data: &[u8], want: &str) -> Result<Vec<u8>, String> {
    let corrupt = || "the zip archive is corrupt".to_string();
    let add = |a: usize, b: usize| a.checked_add(b).ok_or_else(corrupt);
    let span = |at: usize, len: usize| -> Result<&[u8], String> {
        data.get(at..add(at, len)?).ok_or_else(corrupt)
    };
    let search = data.len().saturating_sub(22 + 65535);
    let eocd = (search..data.len().saturating_sub(21))
        .rev()
        .find(|&i| data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(corrupt)?;
    let count = u16_at(data, eocd + 10).ok_or_else(corrupt)?;
    let mut at = u32_at(data, eocd + 16).ok_or_else(corrupt)?;
    for _ in 0..count {
        if span(at, 4)? != [0x50, 0x4b, 0x01, 0x02] {
            return Err(corrupt());
        }
        let method = u16_at(data, add(at, 10)?).ok_or_else(corrupt)?;
        let packed = u32_at(data, add(at, 20)?).ok_or_else(corrupt)?;
        let size = u32_at(data, add(at, 24)?).ok_or_else(corrupt)?;
        let name_len = u16_at(data, add(at, 28)?).ok_or_else(corrupt)?;
        let extra_len = u16_at(data, add(at, 30)?).ok_or_else(corrupt)?;
        let comment_len = u16_at(data, add(at, 32)?).ok_or_else(corrupt)?;
        let local = u32_at(data, add(at, 42)?).ok_or_else(corrupt)?;
        let name = span(add(at, 46)?, name_len)?;
        at = add(add(at, 46)?, name_len + extra_len + comment_len)?;
        if base_name(&String::from_utf8_lossy(name)) != want {
            continue;
        }
        if span(local, 4)? != [0x50, 0x4b, 0x03, 0x04] {
            return Err(corrupt());
        }
        let start = add(
            add(local, 30)?,
            u16_at(data, add(local, 26)?).ok_or_else(corrupt)?
                + u16_at(data, add(local, 28)?).ok_or_else(corrupt)?,
        )?;
        let raw = span(start, packed)?;
        if size as u64 > MAX_BINARY {
            return Err(corrupt());
        }
        return match method {
            0 => Ok(raw.to_vec()),
            8 => {
                let mut out = Vec::with_capacity(size);
                flate2::read::DeflateDecoder::new(raw)
                    .take(MAX_BINARY)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("cannot unpack {want} from the zip archive: {e}"))?;
                if out.len() != size {
                    return Err(corrupt());
                }
                Ok(out)
            }
            m => Err(format!(
                "the zip archive uses compression method {m}, which burrow cannot unpack"
            )),
        };
    }
    Err(format!("the archive does not contain {want}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, body) in files {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..107].copy_from_slice(b"0000755");
            h[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
            h[156] = if name.ends_with('/') { b'5' } else { b'0' };
            h[257..262].copy_from_slice(b"ustar");
            tar.extend_from_slice(&h);
            tar.extend_from_slice(body);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
        tar.resize(tar.len() + 1024, 0);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    pub fn zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, body, deflate) in files {
            let packed = if *deflate {
                let mut e =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
                e.write_all(body).unwrap();
                e.finish().unwrap()
            } else {
                body.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&packed);
            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            central.extend_from_slice(&(body.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let start = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn finds_the_binary_in_a_tar_gz() {
        let big = vec![7u8; 5000];
        let gz = tar_gz(&[
            ("burrow-1.0.0-linux-amd64/", b""),
            ("burrow-1.0.0-linux-amd64/LICENSE", b"MIT"),
            ("burrow-1.0.0-linux-amd64/burrow", &big),
        ]);
        assert_eq!(from_tar_gz(&gz, "burrow").unwrap(), big);
        assert_eq!(from_tar_gz(&gz, "LICENSE").unwrap(), b"MIT");
        assert!(
            from_tar_gz(&gz, "burrow.exe")
                .unwrap_err()
                .contains("does not contain")
        );
    }

    #[test]
    fn rejects_broken_tar_gz() {
        assert!(from_tar_gz(b"not gzip", "burrow").is_err());
        let gz = tar_gz(&[("burrow", &[1u8; 3000])]);
        let mut tar = Vec::new();
        flate2::read::GzDecoder::new(&gz[..])
            .read_to_end(&mut tar)
            .unwrap();
        tar.truncate(1024);
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(&tar).unwrap();
        assert!(
            from_tar_gz(&e.finish().unwrap(), "burrow")
                .unwrap_err()
                .contains("truncated")
        );
    }

    #[test]
    fn finds_the_binary_in_a_zip() {
        let exe = (0..20000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let z = zip(&[
            ("burrow-1.0.0-windows-amd64/LICENSE", b"MIT", false),
            ("burrow-1.0.0-windows-amd64/burrow.exe", &exe, true),
        ]);
        assert_eq!(from_zip(&z, "burrow.exe").unwrap(), exe);
        assert_eq!(from_zip(&z, "LICENSE").unwrap(), b"MIT");
        assert!(from_zip(&z, "burrow").is_err());
    }

    #[test]
    fn rejects_broken_zip() {
        assert!(from_zip(b"PK", "burrow.exe").is_err());
        let z = zip(&[("burrow.exe", &[3u8; 4000], true)]);
        let mut cut = z.clone();
        cut.drain(40..60);
        assert!(from_zip(&cut, "burrow.exe").is_err());
    }
}
