//! High-level "press Play" flow shared by the CLI and the GUI.

use crate::auth::{Account, Accounts};
use crate::install::{prepare_profile, InstallOptions, Installed};
use crate::launch::{build, LaunchCommand, LaunchSpec};
use crate::manifest::VersionManifest;
use crate::paths::Paths;
use crate::perf::PerfPreset;
use crate::profiles::{Profile, QuickPlay, Settings};
use crate::{report, Progress, ProgressSink};
use anyhow::Result;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Child;
use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub quick_play: Option<QuickPlay>,
    pub preset: Option<PerfPreset>,
    pub memory_mb: Option<u64>,
    pub demo: bool,
}

/// Install what is missing and build the command line. Does not start Java.
pub fn plan(
    paths: &Paths,
    settings: &Settings,
    manifest: Option<&VersionManifest>,
    profile: &Profile,
    account: &Account,
    overrides: &Overrides,
    sink: &ProgressSink,
) -> Result<(LaunchCommand, Installed)> {
    let t0 = Instant::now();
    let opts = InstallOptions::from(settings);
    let installed = prepare_profile(paths, manifest, profile, &opts, sink)?;
    let preset = overrides.preset.unwrap_or(profile.extra.perf);
    let memory = overrides
        .memory_mb
        .or(Some(profile.extra.memory_mb).filter(|m| *m > 0));
    let heap = crate::perf::heap_mb(preset, memory, installed.modded);
    let quick_play = overrides
        .quick_play
        .clone()
        .unwrap_or_else(|| profile.extra.quick_play.clone());
    let extra_jvm_args = profile
        .official
        .java_args
        .as_deref()
        .map(split_args)
        .unwrap_or_default();
    let spec = LaunchSpec {
        paths,
        version: &installed.version,
        game_dir: profile.game_dir(paths),
        account,
        java: installed.java.clone(),
        java_major: installed.java_major,
        preset,
        heap_mb: heap,
        extra_jvm_args,
        quick_play,
        resolution: profile
            .official
            .resolution
            .as_ref()
            .map(|r| (r.width, r.height)),
        demo: overrides.demo,
    };
    let cmd = build(&spec)?;
    report(
        sink,
        Progress::Log(format!(
            "Ready in {:.0} ms ({} preset, {heap} MiB heap, Java {})",
            t0.elapsed().as_secs_f64() * 1000.0,
            preset.label(),
            installed.java_major
        )),
    );
    Ok((cmd, installed))
}

/// Refresh the account token if needed (saving the change) and return a copy.
pub fn fresh_account(
    paths: &Paths,
    settings: &Settings,
    accounts: &mut Accounts,
    id: &str,
    sink: &ProgressSink,
) -> Result<Account> {
    let acc = accounts
        .accounts
        .iter_mut()
        .find(|a| a.id == id)
        .ok_or_else(|| anyhow::anyhow!("account not found"))?;
    if acc.is_expired() {
        report(sink, Progress::Phase("Refreshing Microsoft sign-in".into()));
        let client = if settings.msa_client_id.is_empty() {
            None
        } else {
            Some(settings.msa_client_id.as_str())
        };
        if crate::auth::ensure_fresh(acc, client)? {
            let copy = acc.clone();
            accounts.save(paths)?;
            return Ok(copy);
        }
    }
    Ok(acc.clone())
}

/// Forward the child's stdout/stderr to the sink line by line, then wait.
pub fn pump_output(mut child: Child, sink: ProgressSink) -> Result<i32> {
    let mut handles = Vec::new();
    if let Some(out) = child.stdout.take() {
        let s = sink.clone();
        handles.push(std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                report(&s, Progress::Log(line));
            }
        }));
    }
    if let Some(err) = child.stderr.take() {
        let s = sink.clone();
        handles.push(std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                report(&s, Progress::Log(line));
            }
        }));
    }
    let status = child.wait()?;
    for h in handles {
        let _ = h.join();
    }
    Ok(status.code().unwrap_or(-1))
}

/// Split a `javaArgs` string respecting simple double quotes.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Names of singleplayer worlds (save folders) in a game directory.
pub fn list_worlds(game_dir: &Path) -> Vec<String> {
    let mut out: Vec<(std::time::SystemTime, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(game_dir.join("saves")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.join("level.dat").is_file() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    let t = std::fs::metadata(p.join("level.dat"))
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    out.push((t, name.to_string()));
                }
            }
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.0));
    out.into_iter().map(|(_, n)| n).collect()
}

/// Servers from `servers.dat` (NBT). Minimal parser: pulls `ip` and `name`
/// string tags out of the uncompressed file.
pub fn list_servers(game_dir: &Path) -> Vec<(String, String)> {
    let Ok(data) = std::fs::read(game_dir.join("servers.dat")) else {
        return vec![];
    };
    let mut out = Vec::new();
    let mut i = 0;
    let mut name: Option<String> = None;
    let mut ip: Option<String> = None;
    // Scan for TAG_String (0x08) entries with the names "name" / "ip".
    while i + 3 <= data.len() {
        if data[i] == 0x08 {
            let nlen = u16::from_be_bytes([data[i + 1], data[i + 2]]) as usize;
            let ns = i + 3;
            if ns + nlen + 2 <= data.len() {
                let key = &data[ns..ns + nlen];
                if key == b"name" || key == b"ip" {
                    let vlen = u16::from_be_bytes([data[ns + nlen], data[ns + nlen + 1]]) as usize;
                    let vs = ns + nlen + 2;
                    if vs + vlen <= data.len() {
                        let val = String::from_utf8_lossy(&data[vs..vs + vlen]).to_string();
                        if key == b"name" {
                            name = Some(val)
                        } else {
                            ip = Some(val)
                        }
                        if let (Some(n), Some(a)) = (&name, &ip) {
                            out.push((n.clone(), a.clone()));
                            name = None;
                            ip = None;
                        }
                        i = vs + vlen;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn args_split() {
        assert_eq!(
            split_args(r#"-Xmx2G -Dfoo="a b" -Dx"#),
            vec!["-Xmx2G", "-Dfoo=a b", "-Dx"]
        );
    }
}
