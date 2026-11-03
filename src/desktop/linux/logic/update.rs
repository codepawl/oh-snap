use std::io::Read;

/// The release asset for a CPU, as named in SHA256SUMS.txt.
pub fn asset_for(arch: &str) -> &'static str {
    match arch {
        "aarch64" => "oh-snap-linux-aarch64.tar.gz",
        _ => "oh-snap-linux-x86_64.tar.gz",
    }
}

const LARGEST_BINARY: u64 = 256 * 1024 * 1024;

/// The `oh-snap` file inside the release tarball.
pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>, String> {
    let decoder = flate2::read::GzDecoder::new(archive);
    let mut tarball = tar::Archive::new(decoder);
    let entries = tarball.entries().map_err(|error| format!("unreadable update archive: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("unreadable update archive: {error}"))?;
        let is_binary = entry
            .path()
            .ok()
            .and_then(|path| path.file_name().map(|name| name == "oh-snap"))
            .unwrap_or(false);
        if !is_binary {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .take(LARGEST_BINARY)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("unreadable update archive: {error}"))?;
        return Ok(bytes);
    }
    Err("the update archive has no oh-snap file".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tarball(name: &str, content: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, name, content).unwrap();
        let raw = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, &raw).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn extracts_the_binary() {
        assert_eq!(extract_binary(&tarball("oh-snap", b"ELF")).unwrap(), b"ELF");
        assert_eq!(extract_binary(&tarball("./oh-snap", b"ELF2")).unwrap(), b"ELF2");
        assert!(extract_binary(&tarball("other", b"x")).is_err());
        assert!(extract_binary(b"not gzip").is_err());
    }

    #[test]
    fn picks_the_asset() {
        assert_eq!(asset_for("aarch64"), "oh-snap-linux-aarch64.tar.gz");
        assert_eq!(asset_for("x86_64"), "oh-snap-linux-x86_64.tar.gz");
    }
}
