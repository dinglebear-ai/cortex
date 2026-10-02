use std::io;
use std::time::Duration;

const LATEST_RELEASE: &str = "https://api.github.com/repos/dinglebear-ai/cortex/releases/latest";

/// Resolve a stable release on the authoritative repository. A separate runtime
/// makes this synchronous CLI helper safe when called from an async command.
pub(super) fn latest_stable_version() -> io::Result<String> {
    std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .user_agent(concat!("cortex-update/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(io::Error::other)?;
            let mut response = client
                .get(LATEST_RELEASE)
                .send()
                .await
                .map_err(io::Error::other)?
                .error_for_status()
                .map_err(io::Error::other)?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(io::Error::other)? {
                if bytes.len() + chunk.len() > 131_072 {
                    return Err(io::Error::other("latest release response exceeded 128 KiB"));
                }
                bytes.extend_from_slice(&chunk);
            }
            parse_release(&bytes)
        })
    })
    .join()
    .map_err(|_| io::Error::other("latest release resolver panicked"))?
}

fn parse_release(bytes: &[u8]) -> io::Result<String> {
    let release: serde_json::Value = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    if release["draft"].as_bool() != Some(false) || release["prerelease"].as_bool() != Some(false) {
        return Err(io::Error::other(
            "latest update target is not a published stable release",
        ));
    }
    let tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| io::Error::other("release tag missing"))?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    let components = version.split('.').collect::<Vec<_>>();
    if components.len() != 3
        || components
            .iter()
            .any(|value| value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(io::Error::other(
            "latest stable release tag must be a numeric semantic version",
        ));
    }
    let expected_url = format!("https://github.com/dinglebear-ai/cortex/releases/tag/{tag}");
    if release["html_url"].as_str() != Some(expected_url.as_str()) {
        return Err(io::Error::other(
            "latest release identity is outside the authoritative repository",
        ));
    }
    Ok(version.to_string())
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
