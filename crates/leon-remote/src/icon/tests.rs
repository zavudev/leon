//! Tests of icon detection: the rules over synthetic file trees through a
//! scripted runner, the pure readers, and one run of the real script.

use super::*;
use crate::runner::{Output, ScriptedRunner};
use leon_core::{MachineId, MachineKind};
use std::collections::BTreeMap;

type Tree = BTreeMap<String, Vec<u8>>;

fn png() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend(32u32.to_be_bytes());
    bytes.extend(32u32.to_be_bytes());
    bytes.extend([8, 6, 0, 0, 0]);
    bytes
}

fn svg() -> Vec<u8> {
    b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"/>".to_vec()
}

fn ico() -> Vec<u8> {
    let mut bytes = vec![
        0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0, 4, 0, 0, 0, 22, 0, 0, 0,
    ];
    bytes.extend([1, 2, 3, 4]);
    bytes
}

fn tree(files: &[(&str, Vec<u8>)]) -> Tree {
    files
        .iter()
        .map(|(path, bytes)| ((*path).to_owned(), bytes.clone()))
        .collect()
}

fn text(files: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
    files
        .iter()
        .map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec()))
        .collect()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// What the real script would print for this tree: the same walk, the same
/// reading of files, the same checks, with `budget` bytes of images.
fn emulate_scan(files: &Tree, remote: Option<&str>, budget: usize) -> String {
    let mut out = String::from("some login banner\nLEON-ICON 1\n");
    if let Some(remote) = remote {
        out.push_str(&format!("REMOTE {remote}\n"));
    }
    let mut packages = vec![".".to_owned()];
    for parent in ["apps", "packages"] {
        let mut names: Vec<&str> = files
            .keys()
            .filter_map(|path| path.strip_prefix(&format!("{parent}/"))?.split('/').next())
            .collect();
        names.dedup();
        packages.extend(names.into_iter().map(|name| format!("{parent}/{name}")));
    }
    packages.truncate(1 + MAX_PACKAGES);
    let mut used = 0;
    for package in &packages {
        out.push_str(&format!("PKG {package}\n"));
        for (probe, path) in probe_list() {
            let full = in_package(package, &path);
            let Some(bytes) = files.get(&full) else {
                continue;
            };
            match probe {
                Probe::Exists => out.push_str(&format!("EXISTS {full}\n")),
                Probe::Read if bytes.len() <= MAX_ICON_BYTES => {
                    out.push_str(&format!(
                        "FILE R {} {full}\n{}\n",
                        bytes.len(),
                        base64(bytes)
                    ));
                }
                Probe::Image if sniff(bytes).is_some() => {
                    if used + bytes.len() <= budget {
                        used += bytes.len();
                        out.push_str(&format!(
                            "FILE I {} {full}\n{}\n",
                            bytes.len(),
                            base64(bytes)
                        ));
                    } else {
                        out.push_str(&format!("SKIP I {} {full}\n", bytes.len()));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn emulate_fetch(files: &Tree, wanted: &[String]) -> String {
    let mut out = String::from("LEON-ICON 1\n");
    for path in wanted {
        if let Some(bytes) = files.get(path).filter(|b| sniff(b).is_some()) {
            out.push_str(&format!(
                "FILE I {} {path}\n{}\n",
                bytes.len(),
                base64(bytes)
            ));
        }
    }
    out
}

fn local() -> Machine {
    Machine {
        id: MachineId::local(),
        name: "This machine".into(),
        kind: MachineKind::Local,
    }
}

fn remote_machine() -> Machine {
    Machine {
        id: MachineId::from_string("box"),
        name: "box".into(),
        kind: MachineKind::Ssh {
            host: "box.example".into(),
            user: None,
            port: None,
            identity_file: None,
        },
    }
}

/// Runs detection over `files`: the first command answers with the scan, the
/// second (when one is made) with what was asked for.
async fn detect_in(
    files: &Tree,
    remote: Option<&str>,
    machine: &Machine,
) -> (Detection, ScriptedRunner) {
    let runner = ScriptedRunner::new().reply(Output::ok(emulate_scan(files, remote, BUDGET_BYTES)));
    let (detection, calls) = run_detect(runner, files, machine).await;
    (detection, calls)
}

async fn run_detect(
    runner: ScriptedRunner,
    files: &Tree,
    machine: &Machine,
) -> (Detection, ScriptedRunner) {
    // The second reply cannot be known before the first command ran: a runner
    // that answers fetches from the tree.
    struct Fs<'a> {
        inner: ScriptedRunner,
        files: &'a Tree,
        calls: std::sync::Mutex<usize>,
    }
    impl Runner for Fs<'_> {
        async fn run(&self, spec: &CommandSpec) -> Result<Output, RunError> {
            let first = {
                let mut calls = self.calls.lock().unwrap();
                *calls += 1;
                *calls == 1
            };
            if first {
                return self.inner.run(spec).await;
            }
            let script = spec
                .args
                .iter()
                .find(|arg| arg.contains("LEON-ICON"))
                .cloned()
                .unwrap_or_default();
            let list = script
                .split("LIST='")
                .nth(1)
                .and_then(|s| s.split("'\nfor f").next())
                .unwrap_or("");
            let wanted: Vec<String> = list.lines().map(str::to_owned).collect();
            Ok(Output::ok(emulate_fetch(self.files, &wanted)))
        }
    }
    let fs = Fs {
        inner: runner,
        files,
        calls: std::sync::Mutex::new(0),
    };
    let detection = detect(
        &fs,
        machine,
        &SshOptions::without_multiplexing(),
        "/srv/app",
    )
    .await
    .unwrap();
    (detection, fs.inner)
}

struct Case {
    name: &'static str,
    files: Vec<(String, Vec<u8>)>,
    rule: &'static str,
    path: &'static str,
    trips: u8,
}

fn case(
    name: &'static str,
    files: Vec<(String, Vec<u8>)>,
    rule: &'static str,
    path: &'static str,
    trips: u8,
) -> Case {
    Case {
        name,
        files,
        rule,
        path,
        trips,
    }
}

fn with(mut base: Vec<(String, Vec<u8>)>, more: Vec<(&str, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
    base.extend(more.into_iter().map(|(p, b)| (p.to_owned(), b)));
    base
}

fn pkg(deps: &str) -> Vec<(String, Vec<u8>)> {
    text(&[(
        "package.json",
        &format!("{{\"name\":\"x\",\"dependencies\":{{{deps}}}}}"),
    )])
}

#[tokio::test]
async fn every_framework_rule_finds_its_own_conventional_icon() {
    let next = || {
        with(
            pkg("\"next\":\"14\""),
            vec![("app/layout.tsx", b"x".to_vec())],
        )
    };
    let cases = vec![
        case("next app router icon", with(next(), vec![("app/icon.png", png())]), "nextjs-app", "app/icon.png", 1),
        case("next app router favicon.ico", with(next(), vec![("app/favicon.ico", ico())]), "nextjs-app", "app/favicon.ico", 1),
        case("next app router under src", with(pkg("\"next\":\"14\""), vec![("src/app/layout.tsx", b"x".to_vec()), ("src/app/icon.svg", svg())]), "nextjs-app", "src/app/icon.svg", 1),
        case("next app router falls to public", with(next(), vec![("public/favicon.ico", ico())]), "nextjs-app", "public/favicon.ico", 1),
        case("next pages router", with(pkg("\"next\":\"13\""), vec![("pages/_app.tsx", b"x".to_vec()), ("public/favicon.ico", ico())]), "nextjs-pages", "public/favicon.ico", 1),
        case("vite favicon", with(pkg("\"vite\":\"5\",\"react\":\"18\""), vec![("public/favicon.svg", svg())]), "vite", "public/favicon.svg", 1),
        case("vite index.html link", with(
            with(pkg("\"vite\":\"5\""), vec![("public/brand/mark.svg", svg())]),
            text(&[("index.html", "<html><head><link rel=\"icon\" type=\"image/svg+xml\" href=\"/brand/mark.svg\"></head></html>")]).into_iter().map(|(p, b)| (Box::leak(p.into_boxed_str()) as &str, b)).collect()),
            "vite", "public/brand/mark.svg", 2),
        case("vue is vite", with(pkg("\"vue\":\"3\""), vec![("public/favicon.ico", ico())]), "vite", "public/favicon.ico", 1),
        case("astro prefers svg over ico", with(pkg("\"astro\":\"4\""), vec![("public/favicon.ico", ico()), ("public/favicon.svg", svg())]), "astro", "public/favicon.svg", 1),
        case("sveltekit static", with(pkg("\"@sveltejs/kit\":\"2\""), vec![("static/favicon.png", png())]), "sveltekit", "static/favicon.png", 1),
        case("nuxt public", with(pkg("\"nuxt\":\"3\""), vec![("public/favicon.ico", ico())]), "nuxt", "public/favicon.ico", 1),
        case("remix root links", with(
            with(pkg("\"@remix-run/react\":\"2\""), vec![("public/brand/mark.png", png())]),
            vec![("app/root.tsx", b"export const links = () => [{ rel: \"icon\", href: \"/brand/mark.png\" }];".to_vec())]),
            "remix", "public/brand/mark.png", 2),
        case("react router public favicon", with(pkg("\"react-router\":\"7\""), vec![("public/favicon.ico", ico())]), "remix", "public/favicon.ico", 1),
        case("tanstack start root route", with(
            with(pkg("\"@tanstack/react-start\":\"1\""), vec![("public/mark.png", png())]),
            vec![("src/routes/__root.tsx", b"head: () => ({ links: [{ rel: 'icon', href: '/mark.png' }] })".to_vec())]),
            "tanstack-start", "public/mark.png", 2),
        case("angular src favicon", with(pkg("\"@angular/core\":\"17\""), vec![("src/favicon.ico", ico())]), "angular", "src/favicon.ico", 1),
        case("expo declared icon wins", with(
            with(pkg("\"expo\":\"51\""), vec![("assets/custom.png", png()), ("assets/icon.png", png())]),
            vec![("app.json", b"{\"expo\":{\"icon\":\"./assets/custom.png\"}}".to_vec())]),
            "expo", "assets/custom.png", 2),
        case("expo conventional icon", with(pkg("\"expo\":\"51\""), vec![("assets/images/icon.png", png())]), "expo", "assets/images/icon.png", 1),
        case("react native launcher icon", with(pkg("\"react-native\":\"0.74\""), vec![("android/app/src/main/res/mipmap-xxxhdpi/ic_launcher.png", png())]), "react-native", "android/app/src/main/res/mipmap-xxxhdpi/ic_launcher.png", 1),
        case("tauri icons", with(vec![], vec![("src-tauri/tauri.conf.json", b"{}".to_vec()), ("src-tauri/icons/icon.png", png())]), "tauri", "src-tauri/icons/icon.png", 1),
        case("electron build icon", with(pkg("\"electron\":\"30\""), vec![("build/icon.png", png())]), "electron", "build/icon.png", 1),
        case("flutter web favicon", with(vec![], vec![("pubspec.yaml", b"name: app".to_vec()), ("web/favicon.png", png())]), "flutter", "web/favicon.png", 1),
        case("docusaurus logo", with(pkg("\"@docusaurus/core\":\"3\""), vec![("static/img/logo.svg", svg())]), "docusaurus", "static/img/logo.svg", 1),
        case("mintlify favicon", with(
            vec![],
            vec![("docs.json", b"{\"name\":\"d\",\"favicon\":\"/fav.svg\"}".to_vec()), ("fav.svg", svg())]),
            "mintlify", "fav.svg", 2),
        case("web manifest picks the largest", with(
            vec![],
            vec![
                ("public/manifest.json", b"{\"icons\":[{\"src\":\"icons/a.png\",\"sizes\":\"48x48\"},{\"src\":\"icons/b.png\",\"sizes\":\"512x512\"}]}".to_vec()),
                ("public/icons/a.png", png()),
                ("public/icons/b.png", png()),
            ]),
            "generic", "public/icons/b.png", 2),
        case("generic logo", with(vec![], vec![("logo.png", png())]), "generic", "logo.png", 1),
        case("generic github logo", with(vec![], vec![(".github/logo.svg", svg())]), "generic", ".github/logo.svg", 1),
    ];
    for case in cases {
        let files: Tree = case.files.into_iter().collect();
        let (detection, _) = detect_in(&files, None, &local()).await;
        let found = detection
            .icon
            .unwrap_or_else(|| panic!("{}: nothing found", case.name));
        assert_eq!(
            (
                found.rule.as_str(),
                found.path.as_str(),
                detection.round_trips
            ),
            (case.rule, case.path, case.trips),
            "{}",
            case.name
        );
    }
}

#[tokio::test]
async fn an_unknown_project_falls_back_to_the_generic_list_and_a_bare_one_finds_nothing() {
    let files = tree(&[
        ("README.md", b"hi".to_vec()),
        ("icon.webp", {
            let mut b = b"RIFF\x10\0\0\0WEBPVP8 ".to_vec();
            b.extend([0; 8]);
            b
        }),
    ]);
    let (detection, _) = detect_in(&files, None, &local()).await;
    let found = detection.icon.unwrap();
    assert_eq!(
        (found.rule.as_str(), found.path.as_str()),
        ("generic", "icon.webp")
    );
    let (nothing, _) = detect_in(&tree(&[("README.md", b"hi".to_vec())]), None, &local()).await;
    assert_eq!(nothing.icon, None);
    assert_eq!(nothing.round_trips, 1);
}

#[tokio::test]
async fn in_a_monorepo_an_app_that_declares_a_framework_beats_a_generic_hit_at_the_root() {
    let files: Tree = with(
        with(vec![], vec![("logo.png", png())]),
        vec![
            (
                "apps/web/package.json",
                b"{\"dependencies\":{\"next\":\"14\"}}".to_vec(),
            ),
            ("apps/web/app/layout.tsx", b"x".to_vec()),
            ("apps/web/app/icon.png", png()),
            ("apps/docs/logo.svg", svg()),
        ],
    )
    .into_iter()
    .collect();
    let (detection, _) = detect_in(&files, None, &local()).await;
    let found = detection.icon.unwrap();
    assert_eq!(
        (found.rule.as_str(), found.path.as_str()),
        ("nextjs-app", "apps/web/app/icon.png")
    );
}

#[tokio::test]
async fn in_a_monorepo_without_frameworks_the_root_is_tried_before_the_packages() {
    let files = tree(&[("apps/docs/logo.svg", svg()), ("logo.png", png())]);
    let (detection, _) = detect_in(&files, None, &local()).await;
    assert_eq!(detection.icon.unwrap().path, "logo.png");
    let only = tree(&[("packages/ui/icon.png", png())]);
    let (detection, _) = detect_in(&only, None, &local()).await;
    assert_eq!(detection.icon.unwrap().path, "packages/ui/icon.png");
}

#[tokio::test]
async fn packages_beyond_the_cap_are_not_looked_into() {
    let mut files = Tree::new();
    for n in 0..30 {
        files.insert(format!("apps/a{n:02}/x.txt"), b"x".to_vec());
    }
    files.insert("apps/zzz/logo.png".into(), png());
    let (detection, _) = detect_in(&files, None, &local()).await;
    assert_eq!(detection.icon, None);
}

#[tokio::test]
async fn an_image_the_budget_left_out_is_fetched_by_name_in_a_second_command() {
    let files = tree(&[("logo.png", png())]);
    let runner = ScriptedRunner::new().reply(Output::ok(emulate_scan(&files, None, 0)));
    let (detection, _) = run_detect(runner, &files, &local()).await;
    assert_eq!(detection.round_trips, 2);
    assert_eq!(detection.icon.unwrap().path, "logo.png");
}

#[tokio::test]
async fn a_file_that_is_not_what_its_name_says_is_skipped() {
    let output = format!(
        "LEON-ICON 1\nPKG .\nFILE I 5 favicon.png\n{}\nFILE I {} logo.png\n{}\n",
        base64(b"hello"),
        png().len(),
        base64(&png())
    );
    let runner = ScriptedRunner::new().reply(Output::ok(output));
    let detection = detect(
        &runner,
        &local(),
        &SshOptions::without_multiplexing(),
        "/srv/app",
    )
    .await
    .unwrap();
    assert_eq!(detection.icon.unwrap().path, "logo.png");
}

#[tokio::test]
async fn a_remote_machine_gets_the_same_single_command_over_ssh() {
    let files = tree(&[("logo.png", png())]);
    let (detection, runner) = detect_in(
        &files,
        Some("git@github.com:zavu/api.git"),
        &remote_machine(),
    )
    .await;
    assert_eq!(detection.round_trips, 1);
    assert_eq!(detection.remote.unwrap().key(), "github.com/zavu/api");
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "ssh");
    let remote = calls[0].args.last().unwrap();
    assert!(remote.starts_with("exec sh -c "), "{remote}");
    assert!(remote.ends_with("sh /srv/app"), "{remote}");
}

#[tokio::test]
async fn a_failing_command_is_an_error_not_an_empty_result() {
    let runner = ScriptedRunner::new().reply(Output::failed(3, "cannot cd"));
    let error = detect(
        &runner,
        &local(),
        &SshOptions::without_multiplexing(),
        "/gone",
    )
    .await;
    assert!(matches!(error, Err(IconError::Failed(Some(_)))));
    let nothing = ScriptedRunner::new();
    assert!(matches!(
        detect(
            &nothing,
            &local(),
            &SshOptions::without_multiplexing(),
            "/x"
        )
        .await,
        Err(IconError::Run(_))
    ));
    let garbage = ScriptedRunner::new().reply(Output::ok("hello\n"));
    assert!(matches!(
        detect(
            &garbage,
            &local(),
            &SshOptions::without_multiplexing(),
            "/x"
        )
        .await,
        Err(IconError::Failed(None))
    ));
}

// ----- the pure readers ----------------------------------------------------------------------

#[test]
fn links_are_read_from_html_and_from_the_object_form_with_plain_icons_first() {
    let html = r#"<link rel="apple-touch-icon" href="/touch.png"><link href='/fav.ico' rel='shortcut icon'>
        <link rel="stylesheet" href="/a.css">"#;
    assert_eq!(icon_links(html), ["/fav.ico", "/touch.png"]);
    let tsx = r#"links: [{ rel: "stylesheet", href: appCss }, { rel: 'icon', href: '/mark.svg' }]"#;
    assert_eq!(icon_links(tsx), ["/mark.svg"]);
    let jsx = r#"<link rel="icon" href={"/x.png"} />"#;
    assert_eq!(icon_links(jsx), ["/x.png"]);
}

#[test]
fn a_manifest_lists_its_icons_largest_first() {
    let json = r#"{"icons":[{"src":"a.png","sizes":"48x48"},{"src":"b.png","sizes":"192x192 512x512"},{"src":"c.svg","sizes":"any"}]}"#;
    assert_eq!(manifest_icons(json), ["c.svg", "b.png", "a.png"]);
    assert!(manifest_icons("not json").is_empty());
}

#[test]
fn expo_and_mintlify_configs_name_their_icons() {
    assert_eq!(expo_icon(r#"{"expo":{"icon":"./a.png"}}"#), ["./a.png"]);
    assert_eq!(
        expo_icon("export default { icon: './b.png', splash: {} }"),
        ["./b.png"]
    );
    assert_eq!(
        mintlify_icons(r#"{"favicon":"/f.svg","logo":{"light":"/l.svg","dark":"/d.svg"}}"#),
        ["/f.svg", "/l.svg", "/d.svg"]
    );
}

#[test]
fn a_reference_resolves_against_the_public_folders_and_never_leaves_the_package() {
    assert_eq!(
        resolve(".", "", "/brand/mark.svg"),
        [
            "public/brand/mark.svg",
            "static/brand/mark.svg",
            "brand/mark.svg"
        ]
    );
    assert_eq!(
        resolve(".", "public", "icons/a.png"),
        ["public/icons/a.png", "static/icons/a.png", "icons/a.png"]
    );
    assert_eq!(
        resolve("apps/web", "apps/web", "./logo.png")[0],
        "apps/web/logo.png"
    );
    assert!(resolve(".", "", "../outside.png").is_empty());
    assert!(resolve("apps/web", "apps/web", "../../x.png").is_empty());
    assert!(resolve(".", "", "https://cdn.example/x.png").is_empty());
    assert!(resolve(".", "", "//cdn.example/x.png").is_empty());
    assert!(resolve(".", "", "data:image/png;base64,AAAA").is_empty());
    assert!(
        resolve(".", "", "/etc/passwd").is_empty(),
        "not an image name"
    );
    assert_eq!(resolve(".", "", "/logo.png?v=3#x")[0], "public/logo.png");
}

#[test]
fn patterns_expand_to_extensions_in_order() {
    assert_eq!(
        expand("app/icon.*"),
        [
            "app/icon.png",
            "app/icon.svg",
            "app/icon.webp",
            "app/icon.jpg",
            "app/icon.ico"
        ]
    );
    assert_eq!(
        expand("public/favicon.{svg,ico,png}"),
        [
            "public/favicon.svg",
            "public/favicon.ico",
            "public/favicon.png"
        ]
    );
    assert_eq!(expand("app/favicon.ico"), ["app/favicon.ico"]);
}

#[test]
fn every_probed_and_candidate_path_is_a_safe_relative_path() {
    for (_, path) in probe_list() {
        assert!(safe_path(&path), "{path}");
    }
    assert!(!safe_path("../x"));
    assert!(!safe_path("/x"));
    assert!(!safe_path("a/../x"));
    assert!(!safe_path("it's.png"));
}

#[test]
fn base64_is_decoded_with_or_without_padding_and_refuses_junk() {
    assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
    assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
    assert_eq!(base64_decode("").unwrap(), b"");
    assert_eq!(base64_decode("a!b"), None);
    assert_eq!(base64_decode(&base64(&png())).unwrap(), png());
}

#[test]
fn remote_urls_are_read_in_every_form_and_only_github_hosts_have_an_avatar() {
    let key = |url: &str| parse_remote(url).map(|r| r.key());
    assert_eq!(
        key("https://github.com/zavu/leon.git").as_deref(),
        Some("github.com/zavu/leon")
    );
    assert_eq!(
        key("https://user:token@github.com/zavu/leon").as_deref(),
        Some("github.com/zavu/leon")
    );
    assert_eq!(
        key("git@github.com:zavu/leon.git").as_deref(),
        Some("github.com/zavu/leon")
    );
    assert_eq!(
        key("ssh://git@github.com:22/zavu/leon.git").as_deref(),
        Some("github.com/zavu/leon")
    );
    assert_eq!(
        key("ssh://git@ssh.github.com:443/zavu/leon.git").as_deref(),
        Some("github.com/zavu/leon")
    );
    assert_eq!(
        key("git@GitHub.Example.com:team/app").as_deref(),
        Some("github.example.com/team/app")
    );
    assert_eq!(key("nonsense"), None);
    assert_eq!(key("https://github.com/only-owner"), None);

    let avatar = |url: &str| parse_remote(url).and_then(|r| r.avatar_url());
    assert_eq!(
        avatar("git@github.com:zavu/leon.git").as_deref(),
        Some("https://github.com/zavu.png?size=64")
    );
    assert_eq!(
        avatar("https://github.acme.com/team/app").as_deref(),
        Some("https://github.acme.com/team.png?size=64")
    );
    assert_eq!(
        avatar("https://octo.ghe.com/o/r").as_deref(),
        Some("https://octo.ghe.com/o.png?size=64")
    );
    assert_eq!(avatar("https://gitlab.com/zavu/leon"), None);
    assert_eq!(avatar("git@bitbucket.org:zavu/leon.git"), None);
    assert_eq!(
        parse_remote("git@github.com:zavu/leon.git").unwrap().slug(),
        "zavu/leon"
    );
}

// ----- the real script -----------------------------------------------------------------------

/// The one test that runs the real script: the shell, `od`, `base64`, `git`
/// are what the fakes cannot prove. Skipped where there is no POSIX shell.
#[cfg(unix)]
#[test]
fn the_real_script_reports_a_real_tree() {
    let dir = tempfile::tempdir().unwrap();
    let write = |path: &str, bytes: &[u8]| {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, bytes).unwrap();
    };
    write(
        "apps/web/package.json",
        b"{\"dependencies\":{\"next\":\"14\"}}",
    );
    write("apps/web/app/layout.tsx", b"x");
    write("apps/web/app/icon.png", &png());
    write("apps/web/app/favicon.ico", b"this is not an icon");
    write("logo.svg", &svg());
    std::os::unix::fs::symlink("/etc/hosts", dir.path().join("icon.png")).unwrap();
    let output = match std::process::Command::new("sh")
        .args(["-c", &scan_script(), "sh"])
        .arg(dir.path())
        .output()
    {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        Ok(output) => panic!(
            "the script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(_) => return,
    };
    let mut scan = Scan::default();
    assert!(parse_output(&output, &mut scan));
    assert_eq!(scan.packages, [".", "apps/web"]);
    assert!(
        matches!(scan.found.get("apps/web/app/icon.png"), Some(Found::Image(bytes)) if *bytes == png())
    );
    assert!(
        !scan.found.contains_key("apps/web/app/favicon.ico"),
        "failed the magic check"
    );
    assert!(
        !scan.found.contains_key("icon.png"),
        "a symlink is not followed"
    );
    assert!(matches!(scan.found.get("logo.svg"), Some(Found::Image(_))));
    assert!(matches!(
        scan.found.get("apps/web/package.json"),
        Some(Found::Text(_))
    ));
    assert!(scan.has("apps/web/app/layout.tsx"));
    for package in &scan.packages {
        for (_, path) in probe_list() {
            scan.probed.insert(in_package(package, &path));
        }
    }
    let wanted = candidates(&scan);
    match decide(&scan, &wanted, false) {
        Decision::Found(candidate, _) => {
            assert_eq!(
                (candidate.rule, candidate.path.as_str()),
                ("nextjs-app", "apps/web/app/icon.png")
            );
        }
        other => panic!("{other:?}"),
    }
}
