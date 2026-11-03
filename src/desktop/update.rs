//! Self-update from GitHub Releases, the same scheme as the Windows `updater`: the latest tag
//! is where `releases/latest` redirects to, the archive for this OS and `SHA256SUMS.txt`
//! come from `releases/latest/download`, the checksum must match, and then the OS installs it.

use sha2::{Digest, Sha256};

const RELEASES: &str = "https://github.com/codepawl/oh-snap/releases/latest";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateResult {
    UpToDate,
    /// A newer release exists (notify mode).
    Available(String),
    /// The newer release is installed; it runs after a restart.
    Installed(String),
}

/// "v0.5.0" from ".../releases/tag/v0.5.0".
pub fn tag_from_release_url(url: &str) -> Option<String> {
    let tag = url.split_once("/releases/tag/")?.1.trim_end_matches('/');
    (!tag.is_empty() && !tag.contains('/')).then(|| tag.to_string())
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |text: &str| -> Vec<u64> { text.split(['.', '-']).take(3).map(|part| part.parse::<u64>().unwrap_or(0)).collect() };
    parse(candidate) > parse(current)
}

/// The checksum of `asset` in a `sha256sum`-style file: `<hex>  <name>` per line.
pub fn checksum_for(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == asset).then(|| hash.to_ascii_lowercase())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn verify(bytes: &[u8], sums: &str, asset: &str) -> Result<(), String> {
    let expected = checksum_for(sums, asset).ok_or_else(|| format!("{asset} is not in SHA256SUMS.txt"))?;
    let actual = sha256_hex(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(format!("checksum mismatch for {asset}: expected {expected}, got {actual}"))
    }
}

#[cfg(any(test, not(windows)))]
mod network {
    use std::time::Duration;

    use super::{RELEASES, UpdateResult, CURRENT_VERSION, is_newer, tag_from_release_url, verify};
    use crate::desktop::platform::Platform;
    use crate::settings::UpdateMode;

    const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;

    fn agent(redirects: u32) -> ureq::Agent {
        let provider = std::sync::Arc::new(rustls_rustcrypto::provider());
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .unversioned_rustls_crypto_provider(provider)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .max_redirects(redirects)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(120)))
            .build();
        config.into()
    }

    /// The newest release tag, read from where `releases/latest` redirects.
    pub fn latest_tag() -> Result<String, String> {
        let response = agent(0).head(RELEASES).call().map_err(|error| error.to_string())?;
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .ok_or("GitHub did not say where the latest release is")?;
        tag_from_release_url(location).ok_or_else(|| format!("no release tag in {location}"))
    }

    fn download(name: &str) -> Result<Vec<u8>, String> {
        let url = format!("{RELEASES}/download/{name}");
        let mut response = agent(5).get(&url).call().map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("{url}: HTTP {}", response.status()));
        }
        response.body_mut().with_config().limit(MAX_DOWNLOAD_BYTES).read_to_vec().map_err(|error| error.to_string())
    }

    /// Looks for a newer release and, unless the mode is notify-only, installs it.
    pub fn check(platform: &dyn Platform, mode: UpdateMode) -> Result<UpdateResult, String> {
        let tag = latest_tag()?;
        let version = tag.trim_start_matches('v').to_string();
        if !is_newer(&version, CURRENT_VERSION) {
            return Ok(UpdateResult::UpToDate);
        }
        if mode != UpdateMode::Auto {
            return Ok(UpdateResult::Available(version));
        }
        let asset = platform.release_asset();
        let sums = String::from_utf8(download("SHA256SUMS.txt")?).map_err(|_| "SHA256SUMS.txt is not text")?;
        let archive = download(asset)?;
        verify(&archive, &sums, asset)?;
        platform.install_update(&archive)?;
        Ok(UpdateResult::Installed(version))
    }
}

#[cfg(not(windows))]
pub use network::check;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tag_from_the_redirect() {
        assert_eq!(tag_from_release_url("https://github.com/codepawl/oh-snap/releases/tag/v0.12.0").as_deref(), Some("v0.12.0"));
        assert_eq!(tag_from_release_url("https://github.com/codepawl/oh-snap/releases"), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.12.0", "0.11.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.11.0", "0.11.0"));
    }

    #[test]
    fn verifies_against_the_sums_file() {
        let bytes = b"archive";
        let sums = format!("{}  oh-snap-linux-x86_64.tar.gz\n{}  other.zip\n", sha256_hex(bytes), "0".repeat(64));
        assert!(verify(bytes, &sums, "oh-snap-linux-x86_64.tar.gz").is_ok());
        assert!(verify(b"tampered", &sums, "oh-snap-linux-x86_64.tar.gz").unwrap_err().contains("mismatch"));
        assert!(verify(bytes, &sums, "missing.zip").is_err());
    }

    #[test]
    fn sha256_of_abc_is_the_known_value() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}

#[cfg(test)]
mod network_tests {
    /// Needs the internet: `cargo test -- --ignored latest_tag`.
    #[test]
    #[ignore]
    fn latest_tag_comes_from_github_over_pure_rust_tls() {
        let tag = super::network::latest_tag().expect("latest tag");
        assert!(tag.starts_with('v'), "{tag}");
    }
}
