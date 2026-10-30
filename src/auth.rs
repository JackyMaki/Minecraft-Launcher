//! Accounts: Microsoft (device-code OAuth -> Xbox Live -> XSTS -> Minecraft
//! services) and offline accounts.
//!
//! Microsoft login requires an Azure application client id that has been
//! approved for the Minecraft API (see README). It is read from the settings
//! file or the `MC_LAUNCHER_MSA_CLIENT_ID` environment variable.

use crate::http;
use crate::paths::{write_atomic, Paths};
use anyhow::{anyhow, Context, Result};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MSA_DEVICE_CODE_URL: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
pub const MSA_TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
pub const MSA_SCOPE: &str = "XboxLive.signin offline_access";
pub const XBL_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
pub const XSTS_AUTH_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
pub const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
pub const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Microsoft,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Stable id of this account record.
    pub id: String,
    pub kind: AccountKind,
    /// In-game name.
    pub username: String,
    /// Player UUID without dashes.
    pub uuid: String,
    #[serde(default)]
    pub access_token: String,
    /// Unix seconds when `access_token` expires.
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub xuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Accounts {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub selected: Option<String>,
}

impl Accounts {
    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(paths.accounts_json())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        write_atomic(&paths.accounts_json(), text.as_bytes()).context("saving accounts")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                paths.accounts_json(),
                std::fs::Permissions::from_mode(0o600),
            );
        }
        Ok(())
    }

    pub fn selected(&self) -> Option<&Account> {
        let id = self.selected.as_ref()?;
        self.accounts.iter().find(|a| &a.id == id)
    }

    pub fn upsert(&mut self, acc: Account) {
        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|a| a.kind == acc.kind && a.uuid == acc.uuid)
        {
            let id = existing.id.clone();
            *existing = acc;
            existing.id = id.clone();
            self.selected = Some(id);
        } else {
            self.selected = Some(acc.id.clone());
            self.accounts.push(acc);
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.accounts.retain(|a| a.id != id);
        if self.selected.as_deref() == Some(id) {
            self.selected = self.accounts.first().map(|a| a.id.clone());
        }
    }
}

impl Account {
    pub fn offline(name: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            kind: AccountKind::Offline,
            username: name.to_string(),
            uuid: offline_uuid(name),
            access_token: "0".into(),
            expires_at: u64::MAX,
            refresh_token: String::new(),
            xuid: String::new(),
        }
    }

    pub fn user_type(&self) -> &'static str {
        match self.kind {
            AccountKind::Microsoft => "msa",
            AccountKind::Offline => "legacy",
        }
    }

    pub fn is_expired(&self) -> bool {
        self.kind == AccountKind::Microsoft && now() + 60 >= self.expires_at
    }

    /// UUID with dashes, as the game expects on the command line.
    pub fn uuid_dashed(&self) -> String {
        dashed(&self.uuid)
    }
}

pub fn dashed(uuid: &str) -> String {
    let s = uuid.replace('-', "");
    if s.len() != 32 {
        return uuid.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &s[0..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..32]
    )
}

/// Same algorithm as the vanilla server / `UUID.nameUUIDFromBytes("OfflinePlayer:<name>")`.
pub fn offline_uuid(name: &str) -> String {
    let mut h = Md5::new();
    h.update(format!("OfflinePlayer:{name}").as_bytes());
    let mut b: [u8; 16] = h.finalize().into();
    b[6] = (b[6] & 0x0f) | 0x30;
    b[8] = (b[8] & 0x3f) | 0x80;
    crate::download::hex(&b)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Client id compiled into this build (set `MC_LAUNCHER_DEFAULT_CLIENT_ID`
/// at build time; release builds get it from a repository secret).
pub const BUILT_IN_CLIENT_ID: Option<&str> = option_env!("MC_LAUNCHER_DEFAULT_CLIENT_ID");

/// Where the Microsoft client id comes from: environment, then settings,
/// then the id compiled into the binary.
pub fn msa_client_id(settings_client_id: Option<&str>) -> Result<String> {
    if let Ok(v) = std::env::var("MC_LAUNCHER_MSA_CLIENT_ID") {
        if !v.trim().is_empty() {
            return Ok(v.trim().to_string());
        }
    }
    if let Some(v) = settings_client_id {
        if !v.trim().is_empty() {
            return Ok(v.trim().to_string());
        }
    }
    if let Some(v) = BUILT_IN_CLIENT_ID {
        if !v.trim().is_empty() {
            return Ok(v.trim().to_string());
        }
    }
    Err(anyhow!(
        "no Microsoft client id configured. Set `msa_client_id` in the launcher settings (see README: Microsoft login)."
    ))
}

/// First step of the device-code flow: the user visits `verification_uri` and
/// types `user_code`.
#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
    pub message: String,
}

pub fn begin_device_code(client_id: &str) -> Result<DeviceCode> {
    let (status, v) = http::post_form_json(
        MSA_DEVICE_CODE_URL,
        &[("client_id", client_id), ("scope", MSA_SCOPE)],
    )?;
    if status != 200 {
        return Err(anyhow!(
            "device code request failed ({status}): {}",
            describe_oauth_error(&v)
        ));
    }
    Ok(DeviceCode {
        device_code: str_field(&v, "device_code")?,
        user_code: str_field(&v, "user_code")?,
        verification_uri: str_field(&v, "verification_uri")?,
        interval: v["interval"].as_u64().unwrap_or(5),
        expires_in: v["expires_in"].as_u64().unwrap_or(900),
        message: v["message"].as_str().unwrap_or("").to_string(),
    })
}

/// Poll result for the device-code flow.
pub enum Poll {
    Pending,
    Done(MsaTokens),
}

#[derive(Debug, Clone)]
pub struct MsaTokens {
    pub access_token: String,
    pub refresh_token: String,
}

pub fn poll_device_code(client_id: &str, dc: &DeviceCode) -> Result<Poll> {
    let (status, v) = http::post_form_json(
        MSA_TOKEN_URL,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", client_id),
            ("device_code", &dc.device_code),
        ],
    )?;
    if status == 200 {
        return Ok(Poll::Done(MsaTokens {
            access_token: str_field(&v, "access_token")?,
            refresh_token: v["refresh_token"].as_str().unwrap_or("").to_string(),
        }));
    }
    match v["error"].as_str().unwrap_or("") {
        "authorization_pending" | "slow_down" => Ok(Poll::Pending),
        "authorization_declined" => Err(anyhow!("sign-in was declined")),
        "expired_token" => Err(anyhow!("the sign-in code expired, please try again")),
        _ => Err(anyhow!("sign-in failed: {}", describe_oauth_error(&v))),
    }
}

/// Block until the user completes the device-code flow (or it fails).
pub fn wait_device_code(client_id: &str, dc: &DeviceCode) -> Result<MsaTokens> {
    let deadline = std::time::Instant::now() + Duration::from_secs(dc.expires_in);
    let mut interval = dc.interval.max(1);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(interval));
        match poll_device_code(client_id, dc)? {
            Poll::Done(t) => return Ok(t),
            Poll::Pending => interval = (interval + 1).min(15),
        }
    }
    Err(anyhow!("the sign-in code expired"))
}

pub fn refresh_msa(client_id: &str, refresh_token: &str) -> Result<MsaTokens> {
    let (status, v) = http::post_form_json(
        MSA_TOKEN_URL,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("scope", MSA_SCOPE),
        ],
    )?;
    if status != 200 {
        return Err(anyhow!(
            "token refresh failed: {}",
            describe_oauth_error(&v)
        ));
    }
    Ok(MsaTokens {
        access_token: str_field(&v, "access_token")?,
        refresh_token: v["refresh_token"]
            .as_str()
            .unwrap_or(refresh_token)
            .to_string(),
    })
}

/// Exchange Microsoft tokens for a Minecraft account (Xbox Live -> XSTS -> MC).
pub fn complete_login(tokens: MsaTokens) -> Result<Account> {
    // Xbox Live user token
    let body = serde_json::json!({
        "Properties": {"AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com", "RpsTicket": format!("d={}", tokens.access_token)},
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT"
    });
    let (status, v) = http::post_json(XBL_AUTH_URL, &body, &[("x-xbl-contract-version", "1")])?;
    if status != 200 {
        return Err(anyhow!("Xbox Live authentication failed ({status})"));
    }
    let xbl_token = str_field(&v, "Token")?;
    let uhs = v["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .ok_or_else(|| anyhow!("Xbox Live response missing user hash"))?
        .to_string();

    // XSTS
    let body = serde_json::json!({
        "Properties": {"SandboxId": "RETAIL", "UserTokens": [xbl_token]},
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT"
    });
    let (status, v) = http::post_json(XSTS_AUTH_URL, &body, &[("x-xbl-contract-version", "1")])?;
    if status != 200 {
        let xerr = v["XErr"].as_u64().unwrap_or(0);
        let why = match xerr {
            2148916233 => "this Microsoft account has no Xbox profile. Sign in at xbox.com once to create one.",
            2148916235 => "Xbox Live is not available in your country/region.",
            2148916236 | 2148916237 => "this account needs adult verification on xbox.com.",
            2148916238 => "this is a child account; it must be added to a Microsoft family first.",
            _ => "XSTS authorization failed.",
        };
        return Err(anyhow!("{why} (XErr {xerr})"));
    }
    let xsts_token = str_field(&v, "Token")?;

    // Minecraft services
    let body = serde_json::json!({"identityToken": format!("XBL3.0 x={uhs};{xsts_token}")});
    let (status, v) = http::post_json(MC_LOGIN_URL, &body, &[])?;
    if status != 200 {
        return Err(anyhow!("Minecraft login failed ({status}): {v}"));
    }
    let mc_token = str_field(&v, "access_token")?;
    let expires_in = v["expires_in"].as_u64().unwrap_or(86400);

    // Profile (also proves game ownership)
    let (status, p) = http::get_json_auth(MC_PROFILE_URL, &mc_token)?;
    if status == 404 {
        return Err(anyhow!(
            "this Microsoft account does not own Minecraft: Java Edition"
        ));
    }
    if status != 200 {
        return Err(anyhow!("fetching Minecraft profile failed ({status})"));
    }
    let xuid = extract_xuid(&mc_token).unwrap_or_default();
    Ok(Account {
        id: uuid::Uuid::new_v4().simple().to_string(),
        kind: AccountKind::Microsoft,
        username: str_field(&p, "name")?,
        uuid: str_field(&p, "id")?.replace('-', ""),
        access_token: mc_token,
        expires_at: now() + expires_in,
        refresh_token: tokens.refresh_token,
        xuid,
    })
}

/// Make sure the account's Minecraft token is valid, refreshing it if needed.
/// Returns `true` when the account was updated.
pub fn ensure_fresh(acc: &mut Account, client_id: Option<&str>) -> Result<bool> {
    if acc.kind != AccountKind::Microsoft || !acc.is_expired() {
        return Ok(false);
    }
    let client_id = msa_client_id(client_id)?;
    let tokens = refresh_msa(&client_id, &acc.refresh_token)?;
    let fresh = complete_login(tokens)?;
    acc.username = fresh.username;
    acc.uuid = fresh.uuid;
    acc.access_token = fresh.access_token;
    acc.expires_at = fresh.expires_at;
    acc.refresh_token = fresh.refresh_token;
    acc.xuid = fresh.xuid;
    Ok(true)
}

/// The Minecraft access token is a JWT whose payload carries the `xuid` claim.
fn extract_xuid(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64_url_decode(payload)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v["xuid"].as_str().map(|s| s.to_string())
}

fn base64_url_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let c = if c == b'+' {
            b'-'
        } else if c == b'/' {
            b'_'
        } else {
            c
        };
        let v = T.iter().position(|&t| t == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn str_field(v: &serde_json::Value, key: &str) -> Result<String> {
    v[key]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("response missing field '{key}'"))
}

fn describe_oauth_error(v: &serde_json::Value) -> String {
    match (v["error"].as_str(), v["error_description"].as_str()) {
        (Some(e), Some(d)) => format!("{e}: {}", d.lines().next().unwrap_or("")),
        (Some(e), None) => e.to_string(),
        _ => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_vanilla() {
        // Known value: UUID.nameUUIDFromBytes("OfflinePlayer:Notch")
        assert_eq!(
            dashed(&offline_uuid("Notch")),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn jwt_xuid() {
        // header.payload.sig with payload {"xuid":"123"}
        let payload = "eyJ4dWlkIjoiMTIzIn0";
        assert_eq!(
            extract_xuid(&format!("h.{payload}.s")).as_deref(),
            Some("123")
        );
    }
}
