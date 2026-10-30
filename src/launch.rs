//! Builds the final `java ...` command line for a resolved version.

use crate::auth::Account;
use crate::paths::Paths;
use crate::perf::PerfPreset;
use crate::profiles::QuickPlay;
use crate::rules::{allowed, RuleContext};
use crate::version::{Argument, VersionJson};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Everything needed to construct a launch command.
pub struct LaunchSpec<'a> {
    pub paths: &'a Paths,
    pub version: &'a VersionJson,
    pub game_dir: PathBuf,
    pub account: &'a Account,
    pub java: PathBuf,
    pub java_major: u32,
    pub preset: PerfPreset,
    pub heap_mb: u64,
    /// Extra JVM arguments from the profile (`javaArgs`).
    pub extra_jvm_args: Vec<String>,
    pub quick_play: QuickPlay,
    pub resolution: Option<(u32, u32)>,
    pub demo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl LaunchCommand {
    /// Shell-ish rendering for logs (tokens are not exact shell quoting).
    pub fn render(&self, redact_token: Option<&str>) -> String {
        let mut s = shell_quote(&self.program.to_string_lossy());
        for a in &self.args {
            let a = match redact_token {
                Some(t) if !t.is_empty() && t != "0" && a == t => "<access token>".to_string(),
                _ => a.clone(),
            };
            s.push(' ');
            s.push_str(&shell_quote(&a));
        }
        s
    }

    pub fn spawn(&self) -> Result<Child> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The container/agent proxy settings must not leak into the game.
        cmd.env_remove("JAVA_TOOL_OPTIONS");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        Ok(cmd.spawn()?)
    }
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:=,+@%".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Does this version understand the 1.20+ `--quickPlay*` arguments?
pub fn supports_quick_play(version: &VersionJson) -> bool {
    let Some(args) = &version.arguments else {
        return false;
    };
    args.game.iter().any(|a| match a {
        Argument::Conditional { rules, .. } => rules.iter().any(|r| {
            r.features
                .as_ref()
                .map(|f| f.contains_key("has_quick_plays_support"))
                .unwrap_or(false)
        }),
        _ => false,
    })
}

/// Split `host[:port]`, defaulting to 25565.
pub fn split_address(addr: &str) -> (String, u16) {
    let addr = addr.trim();
    if let Some(rest) = addr.strip_prefix('[') {
        // [ipv6]:port
        if let Some((host, port)) = rest.split_once("]:") {
            return (host.to_string(), port.parse().unwrap_or(25565));
        }
        return (rest.trim_end_matches(']').to_string(), 25565);
    }
    match addr.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h.to_string(), p.parse().unwrap_or(25565)),
        _ => (addr.to_string(), 25565),
    }
}

pub fn build(spec: &LaunchSpec) -> Result<LaunchCommand> {
    let v = spec.version;
    let paths = spec.paths;
    let main_class = v
        .main_class
        .clone()
        .ok_or_else(|| anyhow!("version {} has no mainClass", v.id))?;

    // Rule context with features.
    let quick_play_supported = supports_quick_play(v);
    let mut ctx = RuleContext::current()
        .with_feature("is_demo_user", spec.demo)
        .with_feature("has_custom_resolution", spec.resolution.is_some());
    let mut quick_fallback: Vec<String> = Vec::new();
    match &spec.quick_play {
        QuickPlay::None => {}
        QuickPlay::Singleplayer { world } => {
            if quick_play_supported {
                ctx = ctx
                    .with_feature("has_quick_plays_support", true)
                    .with_feature("is_quick_play_singleplayer", true);
            } else {
                log::warn!(
                    "{}: this version predates Quick Play; cannot auto-join world '{world}'",
                    v.id
                );
            }
        }
        QuickPlay::Multiplayer { address } => {
            if quick_play_supported {
                ctx = ctx
                    .with_feature("has_quick_plays_support", true)
                    .with_feature("is_quick_play_multiplayer", true);
            } else {
                let (host, port) = split_address(address);
                quick_fallback = vec!["--server".into(), host, "--port".into(), port.to_string()];
            }
        }
        QuickPlay::Realms { .. } => {
            if quick_play_supported {
                ctx = ctx
                    .with_feature("has_quick_plays_support", true)
                    .with_feature("is_quick_play_realms", true);
            }
        }
    }

    // Classpath.
    let libs = v.library_files(&ctx)?;
    let lib_dir = paths.libraries();
    let mut classpath: Vec<String> = Vec::with_capacity(libs.len() + 1);
    for l in libs.iter().filter(|l| !l.native) {
        classpath.push(lib_dir.join(&l.path).to_string_lossy().into_owned());
    }
    let jar_id = v.jar_id();
    let jar = paths.version_jar(jar_id);
    classpath.push(jar.to_string_lossy().into_owned());
    let classpath_str = classpath.join(crate::paths::classpath_separator());

    let natives = paths.natives_dir(&v.id);
    let assets_index = v
        .asset_index
        .as_ref()
        .map(|a| a.id.clone())
        .or_else(|| v.assets.clone())
        .unwrap_or_else(|| "legacy".into());
    let game_assets = if assets_index == "pre-1.6" {
        spec.game_dir.join("resources")
    } else if assets_index == "legacy"
        || v.is_legacy()
            && !paths
                .asset_indexes()
                .join(format!("{assets_index}.json"))
                .exists()
    {
        paths.asset_virtual(&assets_index)
    } else {
        // For legacy args ${game_assets} should point at the virtual dir when
        // the index is virtual; otherwise the assets root is fine.
        if paths.asset_virtual(&assets_index).is_dir() {
            paths.asset_virtual(&assets_index)
        } else {
            paths.assets()
        }
    };

    let (res_w, res_h) = spec.resolution.unwrap_or((854, 480));
    let (qp_single, qp_multi, qp_realms) = match &spec.quick_play {
        QuickPlay::Singleplayer { world } => (world.clone(), String::new(), String::new()),
        QuickPlay::Multiplayer { address } => (String::new(), address.clone(), String::new()),
        QuickPlay::Realms { id } => (String::new(), String::new(), id.clone()),
        QuickPlay::None => (String::new(), String::new(), String::new()),
    };

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", spec.account.username.clone());
    vars.insert("version_name", v.id.clone());
    vars.insert(
        "game_directory",
        spec.game_dir.to_string_lossy().into_owned(),
    );
    vars.insert("assets_root", paths.assets().to_string_lossy().into_owned());
    vars.insert("game_assets", game_assets.to_string_lossy().into_owned());
    vars.insert("assets_index_name", assets_index.clone());
    vars.insert("auth_uuid", spec.account.uuid.clone());
    vars.insert("auth_access_token", spec.account.access_token.clone());
    vars.insert(
        "auth_session",
        format!("token:{}:{}", spec.account.access_token, spec.account.uuid),
    );
    vars.insert("auth_xuid", spec.account.xuid.clone());
    vars.insert(
        "clientid",
        "MWRmNTYxZTAtODc0MC00M2M0LWJlNzktODA2ZDQ1OGQ4ZmUw".into(),
    );
    vars.insert("user_type", spec.account.user_type().into());
    vars.insert("user_properties", "{}".into());
    vars.insert(
        "version_type",
        v.kind.clone().unwrap_or_else(|| "release".into()),
    );
    vars.insert("resolution_width", res_w.to_string());
    vars.insert("resolution_height", res_h.to_string());
    vars.insert("natives_directory", natives.to_string_lossy().into_owned());
    vars.insert("launcher_name", crate::LAUNCHER_NAME.into());
    vars.insert("launcher_version", crate::LAUNCHER_VERSION.into());
    vars.insert("classpath", classpath_str.clone());
    vars.insert(
        "classpath_separator",
        crate::paths::classpath_separator().into(),
    );
    vars.insert("library_directory", lib_dir.to_string_lossy().into_owned());
    vars.insert("primary_jar", jar.to_string_lossy().into_owned());
    vars.insert("quickPlayPath", "quickPlay/latest.json".into());
    vars.insert("quickPlaySingleplayer", qp_single);
    vars.insert("quickPlayMultiplayer", qp_multi);
    vars.insert("quickPlayRealms", qp_realms);

    // JVM args.
    let mut args: Vec<String> = Vec::new();
    let cds = if spec.preset != PerfPreset::Vanilla {
        Some(
            paths
                .cds_dir()
                .join(format!("{}-java{}.jsa", sanitize(&v.id), spec.java_major)),
        )
    } else {
        None
    };
    if let Some(c) = &cds {
        let _ = std::fs::create_dir_all(c.parent().unwrap_or(Path::new(".")));
    }
    args.extend(crate::perf::jvm_flags(
        spec.preset,
        spec.java_major,
        spec.heap_mb,
        cds.as_deref(),
    ));

    // Version-provided JVM args (or the legacy defaults).
    let mut version_jvm: Vec<String> = Vec::new();
    match &v.arguments {
        Some(a) => version_jvm.extend(expand_args(&a.jvm, &ctx, &vars)),
        None => {
            if cfg!(target_os = "macos") {
                version_jvm.push("-XstartOnFirstThread".into());
            }
            version_jvm.push(format!("-Djava.library.path={}", vars["natives_directory"]));
            version_jvm.push(format!(
                "-Dminecraft.launcher.brand={}",
                crate::LAUNCHER_NAME
            ));
            version_jvm.push(format!(
                "-Dminecraft.launcher.version={}",
                crate::LAUNCHER_VERSION
            ));
            version_jvm.push("-cp".into());
            version_jvm.push(classpath_str.clone());
        }
    }
    if !version_jvm.iter().any(|a| a == "-cp" || a == "-classpath") {
        version_jvm.push("-cp".into());
        version_jvm.push(classpath_str.clone());
    }
    args.extend(version_jvm);

    // Logging configuration (log4j config from Mojang).
    if let Some(client) = v.logging.as_ref().and_then(|l| l.get("client")) {
        let file = paths.log_configs().join(&client.file.id);
        if file.exists() && !client.argument.is_empty() {
            args.push(client.argument.replace("${path}", &file.to_string_lossy()));
        }
    }

    // User's own JVM args last so they override the preset.
    args.extend(spec.extra_jvm_args.iter().cloned());

    args.push(main_class);

    // Game args.
    match (&v.arguments, &v.minecraft_arguments) {
        (Some(a), _) => args.extend(expand_args(&a.game, &ctx, &vars)),
        (None, Some(legacy)) => {
            for tok in legacy.split_whitespace() {
                args.push(substitute(tok, &vars));
            }
        }
        (None, None) => return Err(anyhow!("version {} has no game arguments", v.id)),
    }
    if spec.demo && !args.iter().any(|a| a == "--demo") {
        args.push("--demo".into());
    }
    if spec.resolution.is_some() && !args.iter().any(|a| a == "--width") {
        args.extend([
            "--width".into(),
            res_w.to_string(),
            "--height".into(),
            res_h.to_string(),
        ]);
    }
    args.extend(quick_fallback);

    Ok(LaunchCommand {
        program: spec.java.clone(),
        args,
        cwd: spec.game_dir.clone(),
    })
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn expand_args(list: &[Argument], ctx: &RuleContext, vars: &HashMap<&str, String>) -> Vec<String> {
    let mut out = Vec::new();
    for a in list {
        match a {
            Argument::Plain(s) => out.push(substitute(s, vars)),
            Argument::Conditional { rules, value } => {
                if allowed(rules, ctx) {
                    for s in value.as_vec() {
                        out.push(substitute(&s, vars));
                    }
                }
            }
        }
    }
    out
}

/// Replace every `${name}` occurrence; unknown names are left as-is.
pub fn substitute(s: &str, vars: &HashMap<&str, String>) -> String {
    if !s.contains("${") {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match vars.get(name) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push_str("${");
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitution() {
        let mut v = HashMap::new();
        v.insert("a", "1".to_string());
        assert_eq!(substitute("x${a}y${b}", &v), "x1y${b}");
        assert_eq!(substitute("plain", &v), "plain");
    }

    #[test]
    fn addresses() {
        assert_eq!(
            split_address("mc.hypixel.net"),
            ("mc.hypixel.net".into(), 25565)
        );
        assert_eq!(
            split_address("play.example.org:25566"),
            ("play.example.org".into(), 25566)
        );
        assert_eq!(split_address("[::1]:1234"), ("::1".into(), 1234));
    }
}
