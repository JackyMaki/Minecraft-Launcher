//! End-to-end test: a local mock of Mojang's / Fabric's / Modrinth's servers,
//! a stand-in "Minecraft" jar, and the real `mclauncher` binary.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn sha1(data: &[u8]) -> String {
    minecraft_launcher::download::sha1_hex(data)
}

fn jar(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            z.start_file(*name, o).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

struct Mock {
    base: String,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    hits: Arc<Mutex<Vec<String>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Mock {
    fn start() -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let hits: Arc<Mutex<Vec<String>>> = Arc::default();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Several worker threads so parallel keep-alive downloads are served.
        let mut workers = Vec::new();
        for _ in 0..8 {
            let (server, f, h, s) = (server.clone(), files.clone(), hits.clone(), stop.clone());
            workers.push(std::thread::spawn(move || {
                while !s.load(std::sync::atomic::Ordering::Relaxed) {
                    let Ok(Some(req)) = server.recv_timeout(std::time::Duration::from_millis(50))
                    else {
                        continue;
                    };
                    let path = req.url().split('?').next().unwrap().to_string();
                    h.lock().unwrap().push(path.clone());
                    let body = f.lock().unwrap().get(&path).cloned();
                    match body {
                        Some(b) => req.respond(tiny_http::Response::from_data(b)).unwrap(),
                        None => req
                            .respond(
                                tiny_http::Response::from_string("not found").with_status_code(404),
                            )
                            .unwrap(),
                    }
                }
            }));
        }
        let thread = std::thread::spawn(move || {
            for w in workers {
                let _ = w.join();
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            files,
            hits,
            stop,
            thread: Some(thread),
        }
    }
    fn put(&self, path: &str, data: Vec<u8>) -> (String, String, u64) {
        let (sha, size) = (sha1(&data), data.len() as u64);
        self.files.lock().unwrap().insert(path.to_string(), data);
        (format!("{}{path}", self.base), sha, size)
    }
    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
    fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mclauncher"));
    for (k, _) in std::env::vars() {
        if k.to_ascii_lowercase().contains("proxy") {
            c.env_remove(k);
        }
    }
    c.env_remove("JAVA_TOOL_OPTIONS");
    c
}

fn have_java() -> bool {
    Command::new("java")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn read_args(game_dir: &Path) -> Vec<String> {
    std::fs::read_to_string(game_dir.join("launch_args.txt"))
        .expect("mock Minecraft did not write launch_args.txt")
        .lines()
        .map(|s| s.to_string())
        .collect()
}

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Build all mock content and return the manifest URL.
fn populate(mock: &Mock) -> String {
    let main_class = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mockmc/Main.class"),
    )
    .unwrap();
    let client = jar(&[("Main.class", &main_class[..])]);
    let (client_url, client_sha, client_size) = mock.put("/v1/objects/client.jar", client.clone());

    let lib = jar(&[("lib/marker.txt", b"library")]);
    let (lib_url, lib_sha, lib_size) = mock.put("/libraries/com/mock/lib/1.0/lib-1.0.jar", lib);
    let natives = jar(&[("libmock.so", b"ELF"), ("META-INF/MANIFEST.MF", b"m")]);
    let (nat_url, nat_sha, nat_size) = mock.put(
        "/libraries/org/mock/natives/1.0/natives-1.0-natives-linux.jar",
        natives.clone(),
    );
    let (_nat2_url, _, _) = mock.put(
        "/libraries/org/mock/natives/1.0/natives-1.0-natives-windows.jar",
        natives.clone(),
    );
    let (_nat3_url, _, _) = mock.put(
        "/libraries/org/mock/natives/1.0/natives-1.0-natives-osx.jar",
        natives.clone(),
    );
    // modern style natives entry: only for linux, classifier in the name
    let (modnat_url, modnat_sha, modnat_size) = mock.put(
        "/libraries/org/lwjgl/mocklwjgl/3.3/mocklwjgl-3.3-natives-linux.jar",
        natives.clone(),
    );

    let sound = b"hello sound".to_vec();
    let sound_sha = sha1(&sound);
    mock.put(
        &format!("/resources/{}/{}", &sound_sha[..2], sound_sha),
        sound.clone(),
    );
    let index = serde_json::json!({"objects": {"minecraft/sounds/hello.ogg": {"hash": sound_sha, "size": sound.len()}}});
    let (index_url, index_sha, index_size) =
        mock.put("/indexes/17.json", serde_json::to_vec(&index).unwrap());
    let legacy_index = serde_json::json!({"objects": {"sound/hello.ogg": {"hash": sound_sha, "size": sound.len()}}, "virtual": true});
    let (lindex_url, lindex_sha, lindex_size) = mock.put(
        "/indexes/legacy.json",
        serde_json::to_vec(&legacy_index).unwrap(),
    );
    let (log_url, log_sha, log_size) =
        mock.put("/log/client-1.12.xml", b"<Configuration/>".to_vec());

    let natives_rules = serde_json::json!([{"action":"allow","os":{"name":"linux"}}]);
    let modern = serde_json::json!({
        "id": "mock-1.21",
        "type": "release",
        "mainClass": "Main",
        "assets": "17",
        "assetIndex": {"id": "17", "sha1": index_sha, "size": index_size, "url": index_url},
        "javaVersion": {"component": "java-runtime-delta", "majorVersion": 21},
        "downloads": {"client": {"sha1": client_sha, "size": client_size, "url": client_url}},
        "logging": {"client": {"argument": "-Dlog4j.configurationFile=${path}", "file": {"id": "client-1.12.xml", "sha1": log_sha, "size": log_size, "url": log_url}, "type": "log4j2-xml"}},
        "libraries": [
            {"name": "com.mock:lib:1.0", "downloads": {"artifact": {"path": "com/mock/lib/1.0/lib-1.0.jar", "sha1": lib_sha, "size": lib_size, "url": lib_url}}},
            {"name": "org.lwjgl:mocklwjgl:3.3:natives-linux", "downloads": {"artifact": {"path": "org/lwjgl/mocklwjgl/3.3/mocklwjgl-3.3-natives-linux.jar", "sha1": modnat_sha, "size": modnat_size, "url": modnat_url}}, "rules": natives_rules},
            {"name": "com.mock:maconly:1.0", "downloads": {"artifact": {"path": "com/mock/maconly/1.0/maconly-1.0.jar", "sha1": "0", "size": 1, "url": format!("{}/missing", mock.base)}}, "rules": [{"action":"allow","os":{"name":"osx"}}]}
        ],
        "arguments": {
            "game": [
                "--username", "${auth_player_name}", "--version", "${version_name}", "--gameDir", "${game_directory}",
                "--assetsDir", "${assets_root}", "--assetIndex", "${assets_index_name}", "--uuid", "${auth_uuid}",
                "--accessToken", "${auth_access_token}", "--clientId", "${clientid}", "--xuid", "${auth_xuid}",
                "--userType", "${user_type}", "--versionType", "${version_type}",
                {"rules": [{"action": "allow", "features": {"is_demo_user": true}}], "value": "--demo"},
                {"rules": [{"action": "allow", "features": {"has_custom_resolution": true}}], "value": ["--width", "${resolution_width}", "--height", "${resolution_height}"]},
                {"rules": [{"action": "allow", "features": {"has_quick_plays_support": true}}], "value": ["--quickPlayPath", "${quickPlayPath}"]},
                {"rules": [{"action": "allow", "features": {"is_quick_play_singleplayer": true}}], "value": ["--quickPlaySingleplayer", "${quickPlaySingleplayer}"]},
                {"rules": [{"action": "allow", "features": {"is_quick_play_multiplayer": true}}], "value": ["--quickPlayMultiplayer", "${quickPlayMultiplayer}"]},
                {"rules": [{"action": "allow", "features": {"is_quick_play_realms": true}}], "value": ["--quickPlayRealms", "${quickPlayRealms}"]}
            ],
            "jvm": [
                {"rules": [{"action": "allow", "os": {"name": "osx"}}], "value": ["-XstartOnFirstThread"]},
                {"rules": [{"action": "allow", "os": {"name": "windows"}}], "value": "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump"},
                "-Djava.library.path=${natives_directory}", "-Dminecraft.launcher.brand=${launcher_name}", "-Dminecraft.launcher.version=${launcher_version}",
                "-cp", "${classpath}"
            ]
        }
    });
    let (modern_url, modern_sha, _) = mock.put(
        "/v1/packages/mock-1.21.json",
        serde_json::to_vec(&modern).unwrap(),
    );

    let legacy = serde_json::json!({
        "id": "mock-legacy",
        "type": "old_beta",
        "mainClass": "Main",
        "assets": "legacy",
        "assetIndex": {"id": "legacy", "sha1": lindex_sha, "size": lindex_size, "url": lindex_url},
        "downloads": {"client": {"sha1": client_sha, "size": client_size, "url": client_url}},
        "minecraftArguments": "--username ${auth_player_name} --session ${auth_session} --version ${version_name} --gameDir ${game_directory} --assetsDir ${game_assets}",
        "libraries": [
            {"name": "org.mock:natives:1.0",
             "downloads": {"classifiers": {
                 "natives-linux": {"path": "org/mock/natives/1.0/natives-1.0-natives-linux.jar", "sha1": nat_sha, "size": nat_size, "url": nat_url},
                 "natives-windows": {"path": "org/mock/natives/1.0/natives-1.0-natives-windows.jar", "sha1": nat_sha, "size": nat_size, "url": format!("{}/libraries/org/mock/natives/1.0/natives-1.0-natives-windows.jar", mock.base)},
                 "natives-osx": {"path": "org/mock/natives/1.0/natives-1.0-natives-osx.jar", "sha1": nat_sha, "size": nat_size, "url": format!("{}/libraries/org/mock/natives/1.0/natives-1.0-natives-osx.jar", mock.base)}}},
             "natives": {"linux": "natives-linux", "windows": "natives-windows", "osx": "natives-osx"},
             "extract": {"exclude": ["META-INF/"]}}
        ]
    });
    let (legacy_url, legacy_sha, _) = mock.put(
        "/v1/packages/mock-legacy.json",
        serde_json::to_vec(&legacy).unwrap(),
    );

    // Fabric meta + a maven library served from a "maven" root.
    let fabric_lib = jar(&[("fabric.txt", b"loader")]);
    mock.put(
        "/maven/net/mockfabric/loader/0.1/loader-0.1.jar",
        fabric_lib,
    );
    mock.put(
        "/fabric/versions/loader/mock-1.21",
        serde_json::to_vec(&serde_json::json!([
            {"loader": {"version": "0.2-beta", "stable": false}},
            {"loader": {"version": "0.1", "stable": true}}
        ]))
        .unwrap(),
    );
    let fabric_profile = serde_json::json!({
        "id": "fabric-loader-0.1-mock-1.21",
        "inheritsFrom": "mock-1.21",
        "type": "release",
        "mainClass": "Main",
        "arguments": {"game": [], "jvm": ["-Dmock.lib=${library_directory}"]},
        "libraries": [{"name": "net.mockfabric:loader:0.1", "url": format!("{}/maven/", mock.base)}]
    });
    mock.put(
        "/fabric/versions/loader/mock-1.21/0.1/profile/json",
        serde_json::to_vec(&fabric_profile).unwrap(),
    );

    // Modrinth: every performance mod resolves to a small jar; sodium depends on fabric-api's project id.
    let mod_jar = jar(&[("mod.txt", b"mod")]);
    let (_, mod_sha, mod_size) = mock.put("/mods/generic.jar", mod_jar);
    for (slug, _) in minecraft_launcher::mods::PERF_MODS {
        let project_id = format!("id-{slug}");
        let deps = if *slug == "sodium" {
            serde_json::json!([{"project_id": "id-fabric-api", "dependency_type": "required"}])
        } else {
            serde_json::json!([])
        };
        let body = serde_json::json!([{
            "id": format!("v-{slug}"), "project_id": project_id, "version_number": "1.0", "version_type": "release", "date_published": "2025-01-01T00:00:00Z",
            "files": [{"url": format!("{}/mods/generic.jar", mock.base), "filename": format!("{slug}-1.0.jar"), "primary": true, "size": mod_size, "hashes": {"sha1": mod_sha}}],
            "dependencies": deps
        }]);
        let bytes = serde_json::to_vec(&body).unwrap();
        mock.put(&format!("/modrinth/project/{slug}/version"), bytes.clone());
        mock.put(&format!("/modrinth/project/id-{slug}/version"), bytes);
    }

    let manifest = serde_json::json!({
        "latest": {"release": "mock-1.21", "snapshot": "mock-1.21"},
        "versions": [
            {"id": "mock-1.21", "type": "release", "url": modern_url, "time": "2025-01-01T00:00:00+00:00", "releaseTime": "2025-01-01T00:00:00+00:00", "sha1": modern_sha, "complianceLevel": 1},
            {"id": "mock-legacy", "type": "old_beta", "url": legacy_url, "time": "2011-01-01T00:00:00+00:00", "releaseTime": "2011-01-01T00:00:00+00:00", "sha1": legacy_sha, "complianceLevel": 0}
        ]
    });
    let (manifest_url, _, _) = mock.put(
        "/mc/game/version_manifest_v2.json",
        serde_json::to_vec(&manifest).unwrap(),
    );
    manifest_url
}

fn env_for(cmd: &mut Command, root: &Path, mock: &Mock, manifest_url: &str) {
    cmd.env("MC_LAUNCHER_ROOT", root)
        .env("MC_LAUNCHER_MANIFEST_URL", manifest_url)
        .env(
            "MC_LAUNCHER_RESOURCES_URL",
            format!("{}/resources", mock.base),
        )
        .env(
            "MC_LAUNCHER_FABRIC_META_URL",
            format!("{}/fabric", mock.base),
        )
        .env(
            "MC_LAUNCHER_MODRINTH_URL",
            format!("{}/modrinth", mock.base),
        )
        .env(
            "MC_LAUNCHER_JAVA_MANIFEST_URL",
            format!("{}/java/all.json", mock.base),
        );
}

fn run(cmd: &mut Command) -> (bool, String) {
    let t = Instant::now();
    let out = cmd.output().expect("failed to run mclauncher");
    eprintln!(
        "mclauncher {:?} took {:?}",
        cmd.get_args().take(3).collect::<Vec<_>>(),
        t.elapsed()
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn install_launch_quickplay_fabric_and_warm_start() {
    if !have_java() {
        eprintln!("skipping e2e test: no `java` on PATH");
        return;
    }
    let mut mock = Mock::start();
    let manifest_url = populate(&mock);
    let tmp = tempfile::tempdir().unwrap();
    let root: PathBuf = tmp.path().join("mc");

    // 1. Versions listing hits the manifest.
    let mut c = bin();
    env_for(&mut c, &root, &mock, &manifest_url);
    let (ok, out) = run(c.args(["versions", "--all"]));
    assert!(ok, "{out}");
    assert!(
        out.contains("mock-1.21") && out.contains("mock-legacy"),
        "{out}"
    );

    // 2. Cold launch of the modern version straight into a world, offline account.
    let mut c = bin();
    env_for(&mut c, &root, &mock, &manifest_url);
    let (ok, out) = run(c.args([
        "launch",
        "--version",
        "mock-1.21",
        "--offline",
        "Steve",
        "--world",
        "My World",
        "--preset",
        "smooth",
        "--memory",
        "600",
    ]));
    assert!(ok, "{out}");
    assert!(out.contains("MOCK MINECRAFT OK"), "{out}");
    let args = read_args(&root);
    assert_eq!(arg_after(&args, "--username").as_deref(), Some("Steve"));
    assert_eq!(
        arg_after(&args, "--uuid").as_deref(),
        Some(&minecraft_launcher::auth::offline_uuid("Steve")[..])
    );
    assert_eq!(
        arg_after(&args, "--quickPlaySingleplayer").as_deref(),
        Some("My World")
    );
    assert!(args.iter().any(|a| a == "--quickPlayPath"));
    assert!(!args.iter().any(|a| a == "--demo"));
    assert!(
        !args.iter().any(|a| a == "--width"),
        "no resolution requested"
    );
    assert_eq!(arg_after(&args, "--userType").as_deref(), Some("legacy"));
    let natives = root.join("versions/mock-1.21/natives");
    assert!(
        natives.join("libmock.so").is_file(),
        "modern natives extracted"
    );
    assert!(
        args.iter()
            .any(|l| l == &format!("java.library.path={}", natives.display())),
        "{args:?}"
    );
    assert!(root
        .join("libraries/com/mock/lib/1.0/lib-1.0.jar")
        .is_file());
    assert!(
        !root
            .join("libraries/com/mock/maconly/1.0/maconly-1.0.jar")
            .exists(),
        "mac-only library skipped on linux"
    );
    assert!(root.join("assets/indexes/17.json").is_file());
    assert!(root.join("assets/log_configs/client-1.12.xml").is_file());
    let sound_sha = sha1(b"hello sound");
    assert!(root
        .join("assets/objects")
        .join(&sound_sha[..2])
        .join(&sound_sha)
        .is_file());

    // 3. Legacy version + server: falls back to --server/--port and lays out virtual assets.
    let mut c = bin();
    env_for(&mut c, &root, &mock, &manifest_url);
    let (ok, out) = run(c.args([
        "launch",
        "--version",
        "mock-legacy",
        "--offline",
        "Alex",
        "--server",
        "play.example.org:25566",
        "--preset",
        "vanilla",
        "--memory",
        "600",
    ]));
    assert!(ok, "{out}");
    let args = read_args(&root);
    assert_eq!(
        arg_after(&args, "--server").as_deref(),
        Some("play.example.org")
    );
    assert_eq!(arg_after(&args, "--port").as_deref(), Some("25566"));
    assert_eq!(arg_after(&args, "--username").as_deref(), Some("Alex"));
    assert!(arg_after(&args, "--session")
        .unwrap()
        .starts_with("token:0:"));
    assert_eq!(
        arg_after(&args, "--assetsDir").as_deref(),
        Some(&root.join("assets/virtual/legacy").to_string_lossy()[..])
    );
    assert!(root.join("assets/virtual/legacy/sound/hello.ogg").is_file());
    assert!(root
        .join("versions/mock-legacy/natives/libmock.so")
        .is_file());
    assert!(!root
        .join("versions/mock-legacy/natives/MANIFEST.MF")
        .exists());

    // 4. Fabric profile with the performance mod pack, Quick Play server, custom resolution, demo.
    let mut c = bin();
    env_for(&mut c, &root, &mock, &manifest_url);
    let (ok, out) = run(c.args([
        "profile",
        "add",
        "Fast",
        "--version",
        "latest-release",
        "--fabric",
        "--server",
        "mc.example.net",
        "--resolution",
        "1280x720",
        "--memory",
        "700",
    ]));
    assert!(ok, "{out}");
    let mut c = bin();
    env_for(&mut c, &root, &mock, &manifest_url);
    let (ok, out) = run(c.args(["launch", "Fast", "--offline", "Steve", "--demo"]));
    assert!(ok, "{out}");
    assert!(out.contains("Fabric loader"), "{out}");
    let args = read_args(&root);
    assert_eq!(
        arg_after(&args, "--quickPlayMultiplayer").as_deref(),
        Some("mc.example.net")
    );
    assert_eq!(
        arg_after(&args, "--version").as_deref(),
        Some("fabric-loader-0.1-mock-1.21")
    );
    assert_eq!(arg_after(&args, "--width").as_deref(), Some("1280"));
    assert!(args.iter().any(|a| a == "--demo"));
    assert!(
        args.iter()
            .any(|a| a == &format!("mock.lib={}", root.join("libraries").display())),
        "${{library_directory}} expanded: {args:?}"
    );
    assert!(
        root.join("libraries/net/mockfabric/loader/0.1/loader-0.1.jar")
            .is_file(),
        "maven-style library fetched"
    );
    let mods: Vec<String> = std::fs::read_dir(root.join("mods"))
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".jar"))
        .collect();
    assert_eq!(
        mods.len(),
        minecraft_launcher::mods::PERF_MODS.len(),
        "{mods:?}"
    );
    assert!(mods.contains(&"sodium-1.0.jar".to_string()));
    let fabric_api_queries = mock
        .hits()
        .iter()
        .filter(|h| h.contains("/modrinth/project/") && h.contains("fabric-api"))
        .count();
    assert!(fabric_api_queries >= 1);

    // Profiles file is compatible with the official launcher format.
    let lp: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("launcher_profiles.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(lp["version"], 3);
    let fast = lp["profiles"]
        .as_object()
        .unwrap()
        .values()
        .find(|p| p["name"] == "Fast")
        .unwrap();
    assert_eq!(fast["lastVersionId"], "latest-release");
    assert_eq!(fast["resolution"]["width"], 1280);

    // 5. Warm start with the network gone: dry-run must succeed quickly without any request.
    mock.stop();
    let hits_before = mock.hits().len();
    let t = Instant::now();
    let mut c = bin();
    env_for(&mut c, &root, &mock, &manifest_url);
    let (ok, out) = run(c.args(["launch", "Fast", "--offline", "Steve", "--dry-run"]));
    let elapsed = t.elapsed();
    assert!(ok, "{out}");
    assert!(out.contains("Ready in"), "{out}");
    assert!(
        out.contains("--quickPlayMultiplayer mc.example.net"),
        "{out}"
    );
    assert_eq!(
        mock.hits().len(),
        hits_before,
        "warm start made network requests"
    );
    eprintln!("warm dry-run took {elapsed:?}");
    assert!(elapsed.as_secs() < 5, "warm start too slow: {elapsed:?}");
}
