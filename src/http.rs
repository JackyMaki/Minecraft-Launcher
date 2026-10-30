//! Small HTTP layer on top of `ureq` with connection pooling, proxy support
//! (from `HTTPS_PROXY`/`HTTP_PROXY`) and a tiny on-disk JSON cache so the
//! launcher can start instantly and refresh metadata in the background.

use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

pub fn user_agent() -> String {
    format!(
        "JackyMaki/EloLauncher/{} (https://github.com/JackyMaki/Minecraft-Launcher)",
        crate::LAUNCHER_VERSION
    )
}

/// Shared agent: keep-alive connections, generous pool for parallel downloads.
pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(&user_agent())
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .max_idle_connections(64)
            .max_idle_connections_per_host(32)
            .try_proxy_from_env(true)
            .build()
    })
}

pub fn get_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = agent()
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .with_context(|| format!("reading body of {url}"))?;
    Ok(buf)
}

pub fn get_string(url: &str) -> Result<String> {
    let b = get_bytes(url)?;
    String::from_utf8(b).map_err(|e| anyhow!("{url}: body is not UTF-8: {e}"))
}

pub fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    let text = get_string(url)?;
    serde_json::from_str(&text).with_context(|| format!("parsing JSON from {url}"))
}

/// GET JSON with a disk cache. `max_age` controls how long the cached copy is
/// served without hitting the network. If the network fails and a cached copy
/// exists (of any age), the cached copy is returned.
pub fn get_json_cached<T: DeserializeOwned>(
    url: &str,
    cache_file: &Path,
    max_age: Duration,
) -> Result<T> {
    if let Some(text) = read_cache(cache_file, max_age) {
        if let Ok(v) = serde_json::from_str::<T>(&text) {
            return Ok(v);
        }
    }
    match get_string(url) {
        Ok(text) => {
            let v: T =
                serde_json::from_str(&text).with_context(|| format!("parsing JSON from {url}"))?;
            let _ = crate::paths::write_atomic(cache_file, text.as_bytes());
            Ok(v)
        }
        Err(e) => {
            if let Ok(text) = std::fs::read_to_string(cache_file) {
                if let Ok(v) = serde_json::from_str::<T>(&text) {
                    log::warn!("{url}: network failed ({e}); using stale cache");
                    return Ok(v);
                }
            }
            Err(e)
        }
    }
}

/// Read the cached file if it is younger than `max_age`.
pub fn read_cache(cache_file: &Path, max_age: Duration) -> Option<String> {
    let meta = std::fs::metadata(cache_file).ok()?;
    let modified = meta.modified().ok()?;
    if modified.elapsed().unwrap_or(Duration::MAX) > max_age {
        return None;
    }
    std::fs::read_to_string(cache_file).ok()
}

/// Read a cached file regardless of age.
pub fn read_cache_any_age<T: DeserializeOwned>(cache_file: &Path) -> Option<T> {
    let text = std::fs::read_to_string(cache_file).ok()?;
    serde_json::from_str(&text).ok()
}

/// POST a form body and parse the JSON reply. Returns the status code along
/// with the parsed value so callers can inspect OAuth style soft errors.
pub fn post_form_json(url: &str, form: &[(&str, &str)]) -> Result<(u16, serde_json::Value)> {
    let resp = agent().post(url).send_form(form);
    handle_json_response(url, resp)
}

/// POST a JSON body (optionally with extra headers) and parse the JSON reply.
pub fn post_json(
    url: &str,
    body: &serde_json::Value,
    headers: &[(&str, &str)],
) -> Result<(u16, serde_json::Value)> {
    let mut req = agent().post(url).set("Accept", "application/json");
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.send_json(body);
    handle_json_response(url, resp)
}

pub fn get_json_auth(url: &str, bearer: &str) -> Result<(u16, serde_json::Value)> {
    let resp = agent()
        .get(url)
        .set("Authorization", &format!("Bearer {bearer}"))
        .set("Accept", "application/json")
        .call();
    handle_json_response(url, resp)
}

fn handle_json_response(
    url: &str,
    resp: std::result::Result<ureq::Response, ureq::Error>,
) -> Result<(u16, serde_json::Value)> {
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Err(anyhow!("{url}: {e}")),
    };
    let status = resp.status();
    let text = resp.into_string().unwrap_or_default();
    let v = if text.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text))
    };
    Ok((status, v))
}
