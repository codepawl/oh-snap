use std::path::PathBuf;

/// `file:///a%20b/c.png` to `/a b/c.png`. Anything that is not a local file gives None.
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path_start = rest.find('/')?;
    let host = &rest[..path_start];
    if !host.is_empty() && host != "localhost" {
        return None;
    }
    let encoded = rest[path_start..].as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'%' && index + 2 < encoded.len() {
            let pair = std::str::from_utf8(&encoded[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(pair, 16).ok()?);
            index += 3;
        } else {
            decoded.push(encoded[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

/// `/a b/c.png` to `file:///a%20b/c.png`.
pub fn path_to_file_uri(path: &str) -> String {
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_file_uris() {
        assert_eq!(path_to_file_uri("/a b/é.png"), "file:///a%20b/%C3%A9.png");
        assert_eq!(file_uri_to_path(&path_to_file_uri("/x y/%z.png")), Some(PathBuf::from("/x y/%z.png")));
    }

    #[test]
    fn decodes_file_uris() {
        assert_eq!(file_uri_to_path("file:///tmp/a%20b.png"), Some(PathBuf::from("/tmp/a b.png")));
        assert_eq!(file_uri_to_path("file://localhost/x.png"), Some(PathBuf::from("/x.png")));
        assert_eq!(file_uri_to_path("file://other/x.png"), None);
        assert_eq!(file_uri_to_path("https://x/y.png"), None);
        assert_eq!(file_uri_to_path("file:///100%"), Some(PathBuf::from("/100%")));
        assert_eq!(file_uri_to_path("file:///e%C3%A9.png"), Some(PathBuf::from("/eé.png")));
    }
}
