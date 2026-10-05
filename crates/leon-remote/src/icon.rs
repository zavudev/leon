//! Finding a project's logo on the machine it lives on.
//!
//! Detection follows each framework's own conventions. A table of ordered
//! [`IconRule`]s says how a framework is recognised (a file, or a dependency
//! of `package.json`) and where it keeps its icon (candidate files, and files
//! that *declare* one: an `index.html` link, a web app manifest, an Expo
//! config). The rules are evaluated for the project root and, one level down,
//! for `apps/*` and `packages/*` (at most [`MAX_PACKAGES`] directories); a
//! package that declares a real framework is preferred over a generic hit.
//! The generic list (`logo.*`, `icon.*`, `favicon.*`, ...) is the fallback.
//!
//! One command does the looking, so it works the same on an SSH machine: a
//! small POSIX script run through [`run_on`] reports which of the probed
//! files exist, hands over the small text files the rules read (capped at
//! [`MAX_ICON_BYTES`]) and the images that pass a magic-byte check, base64
//! encoded, within a total budget. When a config file points at an icon that
//! was not probed (`index.html` linking `/brand/mark.svg`), one more command
//! fetches those paths; detection therefore makes one round trip, two at the
//! most. Nothing here reads the file system of the machine Leon runs on.
//!
//! The origin remote is read in the same command; [`RemoteRepo`] is what is
//! made of it, and [`RemoteRepo::avatar_url`] is where the owner's avatar is
//! when the host is GitHub or a GitHub Enterprise host.

use std::collections::{HashMap, HashSet};

use leon_core::icon::{sniff, IconImage, MAX_ICON_BYTES};
use leon_core::Machine;
use thiserror::Error;

use crate::command::{run_on, CommandSpec, SshOptions};
use crate::runner::{RunError, Runner};

/// How many package directories (`apps/*`, `packages/*`) are looked into, the
/// root not counted.
pub const MAX_PACKAGES: usize = 16;

/// The most image bytes one command hands back; a valid image beyond it is
/// fetched by name in the second command.
const BUDGET_BYTES: usize = 1_536 * 1_024;

/// The extensions a `*` stands for, in order of preference.
const DEFAULT_EXTS: [&str; 5] = ["png", "svg", "webp", "jpg", "ico"];

/// The extensions an icon file may have.
const ICON_EXTS: [&str; 6] = ["png", "svg", "webp", "jpg", "jpeg", "ico"];

// ----- the rules --------------------------------------------------------------------------

/// How a framework is recognised in one package.
#[derive(Debug, Clone, Copy)]
pub enum Marker {
    /// `package.json` lists this dependency (of any kind).
    Dep(&'static str),
    /// This file exists in the package.
    File(&'static str),
    /// All of these hold.
    All(&'static [Marker]),
    /// At least one of these holds.
    Any(&'static [Marker]),
    /// Always: the generic rule.
    Always,
}

/// A file that names the icon instead of being one.
#[derive(Debug, Clone, Copy)]
pub enum Declared {
    /// `<link rel="icon" href>` in HTML and JSX, or `{ rel: 'icon', href }`
    /// in a route module.
    Links(&'static [&'static str]),
    /// A web app manifest: the largest of its `icons`.
    Manifest(&'static [&'static str]),
    /// An Expo config: `expo.icon`.
    Expo(&'static [&'static str]),
    /// A Mintlify config: `favicon` and `logo`.
    Mintlify(&'static [&'static str]),
}

impl Declared {
    fn files(&self) -> &'static [&'static str] {
        match self {
            Self::Links(files)
            | Self::Manifest(files)
            | Self::Expo(files)
            | Self::Mintlify(files) => files,
        }
    }
}

/// How one kind of project keeps its icon.
#[derive(Debug, Clone, Copy)]
pub struct IconRule {
    /// A short stable name, recorded with the result: `nextjs-app`.
    pub id: &'static str,
    /// How the framework is recognised.
    pub marker: Marker,
    /// Icon files in order of priority. `name.*` stands for each of
    /// [`DEFAULT_EXTS`]; `name.{svg,ico}` lists the extensions.
    pub candidates: &'static [&'static str],
    /// Files that declare an icon.
    pub declared: &'static [Declared],
    /// Whether what the declaring files name comes before `candidates`.
    pub declared_first: bool,
}

const LINK_FILES: &[&str] = &[
    "index.html",
    "public/index.html",
    "src/index.html",
    "src/app.html",
    "app/root.tsx",
    "src/root.tsx",
    "app/routes/__root.tsx",
    "src/routes/__root.tsx",
    "src/routes/__root.jsx",
    "app/layout.tsx",
    "src/app/layout.tsx",
];
const MANIFESTS: &[&str] = &[
    "manifest.json",
    "manifest.webmanifest",
    "site.webmanifest",
    "public/manifest.json",
    "public/manifest.webmanifest",
    "public/site.webmanifest",
    "static/manifest.json",
    "static/manifest.webmanifest",
    "static/site.webmanifest",
];
const WEB_PUBLIC: &[&str] = &["public/favicon.*", "public/icon.*", "public/logo.*"];

/// The framework rules, most specific first. A package takes the first rule
/// whose marker holds, else [`GENERIC`].
pub static RULES: &[IconRule] = &[
    IconRule {
        id: "nextjs-app",
        marker: Marker::All(&[
            Marker::Dep("next"),
            Marker::Any(&[
                Marker::File("app/layout.tsx"),
                Marker::File("app/layout.jsx"),
                Marker::File("app/layout.js"),
                Marker::File("src/app/layout.tsx"),
                Marker::File("src/app/layout.jsx"),
                Marker::File("src/app/layout.js"),
            ]),
        ]),
        candidates: &[
            "app/icon.*",
            "app/favicon.ico",
            "app/apple-icon.png",
            "src/app/icon.*",
            "src/app/favicon.ico",
            "src/app/apple-icon.png",
            "public/favicon.*",
            "public/icon.*",
            "public/logo.*",
        ],
        declared: &[Declared::Manifest(MANIFESTS)],
        declared_first: false,
    },
    IconRule {
        id: "nextjs-pages",
        marker: Marker::Dep("next"),
        candidates: WEB_PUBLIC,
        declared: &[Declared::Links(LINK_FILES), Declared::Manifest(MANIFESTS)],
        declared_first: false,
    },
    IconRule {
        id: "tanstack-start",
        marker: Marker::Any(&[
            Marker::Dep("@tanstack/react-start"),
            Marker::Dep("@tanstack/start"),
            Marker::Dep("@tanstack/solid-start"),
        ]),
        candidates: WEB_PUBLIC,
        declared: &[Declared::Links(LINK_FILES), Declared::Manifest(MANIFESTS)],
        declared_first: true,
    },
    IconRule {
        id: "remix",
        marker: Marker::Any(&[
            Marker::Dep("@remix-run/react"),
            Marker::Dep("@remix-run/node"),
            Marker::Dep("@react-router/dev"),
            Marker::Dep("react-router"),
        ]),
        candidates: WEB_PUBLIC,
        declared: &[Declared::Links(LINK_FILES), Declared::Manifest(MANIFESTS)],
        declared_first: true,
    },
    IconRule {
        id: "astro",
        marker: Marker::Any(&[
            Marker::Dep("astro"),
            Marker::File("astro.config.mjs"),
            Marker::File("astro.config.ts"),
        ]),
        candidates: &[
            "public/favicon.{svg,ico,png}",
            "public/icon.*",
            "public/logo.*",
            "src/assets/logo.*",
        ],
        declared: &[Declared::Manifest(MANIFESTS)],
        declared_first: false,
    },
    IconRule {
        id: "sveltekit",
        marker: Marker::Dep("@sveltejs/kit"),
        candidates: &["static/favicon.*", "static/icon.*", "static/logo.*"],
        declared: &[Declared::Links(LINK_FILES), Declared::Manifest(MANIFESTS)],
        declared_first: true,
    },
    IconRule {
        id: "nuxt",
        marker: Marker::Any(&[Marker::Dep("nuxt"), Marker::File("nuxt.config.ts")]),
        candidates: &[
            "public/favicon.*",
            "public/icon.*",
            "public/logo.*",
            "app/public/favicon.*",
        ],
        declared: &[Declared::Manifest(MANIFESTS)],
        declared_first: false,
    },
    IconRule {
        id: "angular",
        marker: Marker::Any(&[Marker::Dep("@angular/core"), Marker::File("angular.json")]),
        candidates: &[
            "src/favicon.ico",
            "public/favicon.ico",
            "src/favicon.*",
            "public/favicon.*",
            "src/assets/logo.*",
            "src/assets/icons/icon-512x512.png",
        ],
        declared: &[Declared::Links(LINK_FILES), Declared::Manifest(MANIFESTS)],
        declared_first: false,
    },
    IconRule {
        id: "expo",
        marker: Marker::Any(&[
            Marker::Dep("expo"),
            Marker::File("app.json"),
            Marker::File("app.config.js"),
            Marker::File("app.config.ts"),
        ]),
        candidates: &[
            "assets/icon.png",
            "assets/images/icon.png",
            "assets/images/adaptive-icon.png",
            "assets/adaptive-icon.png",
        ],
        declared: &[Declared::Expo(&[
            "app.json",
            "app.config.json",
            "app.config.js",
            "app.config.ts",
        ])],
        declared_first: true,
    },
    IconRule {
        id: "react-native",
        marker: Marker::Dep("react-native"),
        candidates: &[
            "assets/icon.png",
            "assets/images/icon.png",
            "android/app/src/main/res/mipmap-xxxhdpi/ic_launcher.png",
            "android/app/src/main/res/mipmap-xxhdpi/ic_launcher.png",
        ],
        declared: &[],
        declared_first: false,
    },
    IconRule {
        id: "tauri",
        marker: Marker::Any(&[
            Marker::File("src-tauri/tauri.conf.json"),
            Marker::File("src-tauri/Cargo.toml"),
            Marker::Dep("@tauri-apps/cli"),
            Marker::Dep("@tauri-apps/api"),
        ]),
        candidates: &[
            "src-tauri/icons/icon.png",
            "src-tauri/icons/128x128.png",
            "src-tauri/icons/128x128@2x.png",
            "src-tauri/icons/32x32.png",
            "src-tauri/icons/icon.ico",
            "app-icon.png",
        ],
        declared: &[],
        declared_first: false,
    },
    IconRule {
        id: "electron",
        marker: Marker::Any(&[
            Marker::Dep("electron"),
            Marker::Dep("electron-builder"),
            Marker::Dep("electron-vite"),
        ]),
        candidates: &[
            "build/icon.png",
            "resources/icon.png",
            "assets/icon.png",
            "build/icons/512x512.png",
            "build/icon.ico",
            "public/favicon.*",
        ],
        declared: &[],
        declared_first: false,
    },
    IconRule {
        id: "flutter",
        marker: Marker::File("pubspec.yaml"),
        candidates: &[
            "web/favicon.png",
            "web/icons/Icon-192.png",
            "web/icons/Icon-512.png",
            "assets/icon/icon.png",
            "assets/icon.png",
        ],
        declared: &[],
        declared_first: false,
    },
    IconRule {
        id: "docusaurus",
        marker: Marker::Any(&[
            Marker::Dep("@docusaurus/core"),
            Marker::File("docusaurus.config.js"),
            Marker::File("docusaurus.config.ts"),
        ]),
        candidates: &[
            "static/img/logo.*",
            "static/img/favicon.*",
            "static/img/icon.*",
            "static/favicon.*",
        ],
        declared: &[],
        declared_first: false,
    },
    IconRule {
        id: "mintlify",
        marker: Marker::Any(&[Marker::File("docs.json"), Marker::File("mint.json")]),
        candidates: &[
            "favicon.*",
            "logo/light.*",
            "logo/dark.*",
            "logo.*",
            "images/logo.*",
        ],
        declared: &[Declared::Mintlify(&["docs.json", "mint.json"])],
        declared_first: true,
    },
    IconRule {
        id: "vite",
        marker: Marker::Any(&[
            Marker::Dep("vite"),
            Marker::Dep("react-scripts"),
            Marker::Dep("@vitejs/plugin-react"),
            Marker::Dep("vue"),
            Marker::Dep("solid-js"),
            Marker::File("vite.config.ts"),
            Marker::File("vite.config.js"),
            Marker::File("vite.config.mjs"),
        ]),
        candidates: &[
            "public/favicon.*",
            "public/logo.*",
            "public/icon.*",
            "src/assets/logo.*",
            "src/favicon.*",
        ],
        declared: &[Declared::Links(LINK_FILES), Declared::Manifest(MANIFESTS)],
        declared_first: false,
    },
];

/// What is tried in a package no framework rule recognises, and after the
/// frameworks everywhere.
pub static GENERIC: IconRule = IconRule {
    id: "generic",
    marker: Marker::Always,
    candidates: &[
        "favicon.*",
        "public/favicon.*",
        "app/favicon.*",
        "app/icon.*",
        "src/favicon.*",
        "src/app/icon.*",
        "assets/favicon.*",
        "assets/icon.*",
        "static/favicon.*",
        "logo.*",
        "public/logo.*",
        "public/icon.*",
        "assets/logo.*",
        "assets/images/logo.*",
        ".github/logo.*",
        ".github/icon.*",
        "src-tauri/icons/icon.png",
        "app-icon.png",
        "icon.*",
    ],
    declared: &[Declared::Links(LINK_FILES), Declared::Manifest(MANIFESTS)],
    declared_first: false,
};

fn all_rules() -> impl Iterator<Item = &'static IconRule> {
    RULES.iter().chain(std::iter::once(&GENERIC))
}

// ----- path patterns -----------------------------------------------------------------------

/// The files a candidate pattern stands for, in order.
pub fn expand(pattern: &str) -> Vec<String> {
    if let Some(stem) = pattern.strip_suffix(".*") {
        return DEFAULT_EXTS
            .iter()
            .map(|ext| format!("{stem}.{ext}"))
            .collect();
    }
    if let (Some(open), Some(close)) = (pattern.find('{'), pattern.find('}')) {
        if open < close {
            return pattern[open + 1..close]
                .split(',')
                .map(|ext| format!("{}{ext}{}", &pattern[..open], &pattern[close + 1..]))
                .collect();
        }
    }
    vec![pattern.to_owned()]
}

/// Whether `path` stays inside the project: relative, no `..`, no odd
/// characters, and an icon extension when `icon` is asked for.
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', '\n', '\r', '\0', '\''])
        && path
            .split('/')
            .all(|part| part != ".." && part != "." && !part.is_empty())
}

fn has_icon_ext(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ICON_EXTS.contains(&ext.to_lowercase().as_str()))
}

/// A path joined to a base and normalised; `None` when it climbs out of
/// `package` (a relative path of the package, `.` for the root).
fn join_within(package: &str, base: &str, reference: &str) -> Option<String> {
    let mut parts: Vec<&str> = base
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    for part in reference.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    let inside = package == "." || joined == package || joined.starts_with(&format!("{package}/"));
    (inside && !joined.is_empty() && safe_path(&joined)).then_some(joined)
}

fn in_package(package: &str, relative: &str) -> String {
    if package == "." {
        relative.to_owned()
    } else {
        format!("{package}/{relative}")
    }
}

// ----- what is probed ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// Does it exist.
    Exists,
    /// Its text.
    Read,
    /// It, when it is an image.
    Image,
}

impl Probe {
    fn tag(self) -> char {
        match self {
            Self::Exists => 'E',
            Self::Read => 'R',
            Self::Image => 'I',
        }
    }
}

fn marker_files(marker: &Marker, out: &mut Vec<&'static str>) {
    match marker {
        Marker::File(file) => out.push(file),
        Marker::All(all) | Marker::Any(all) => all.iter().for_each(|m| marker_files(m, out)),
        Marker::Dep(_) | Marker::Always => {}
    }
}

/// Every path of a package the first command looks at, with what it asks.
fn probe_list() -> Vec<(Probe, String)> {
    let mut list: Vec<(Probe, String)> = vec![(Probe::Read, "package.json".to_owned())];
    for rule in all_rules() {
        let mut files = Vec::new();
        marker_files(&rule.marker, &mut files);
        list.extend(files.into_iter().map(|f| (Probe::Exists, f.to_owned())));
        for declared in rule.declared {
            list.extend(
                declared
                    .files()
                    .iter()
                    .map(|f| (Probe::Read, (*f).to_owned())),
            );
        }
        for pattern in rule.candidates {
            list.extend(expand(pattern).into_iter().map(|f| (Probe::Image, f)));
        }
    }
    // One entry per path; reading wins over looking at an image over existing.
    let rank = |probe: Probe| match probe {
        Probe::Read => 0,
        Probe::Image => 1,
        Probe::Exists => 2,
    };
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut unique: Vec<(Probe, String)> = Vec::new();
    for (probe, path) in list {
        match seen.get(&path) {
            Some(&at) => {
                if rank(probe) < rank(unique[at].0) {
                    unique[at].0 = probe;
                }
            }
            None => {
                seen.insert(path.clone(), unique.len());
                unique.push((probe, path));
            }
        }
    }
    unique
}

// ----- the commands -----------------------------------------------------------------------

/// The shell the commands share: the helpers that report one file.
const PRELUDE: &str = r#"cd "$1" 2>/dev/null || exit 3
printf 'LEON-ICON 1\n'
NL='
'
CAP=@CAP@
BUDGET=@BUDGET@
used=0
b64() { base64 < "$1" | tr -d '\n\r'; }
size_of() { wc -c < "$1" | tr -d ' '; }
magic_ok() {
  h=$(head -c 16 "$1" | od -An -v -tx1 | tr -d ' \n')
  case "$h" in
    89504e470d0a1a0a*) return 0;;
    52494646????????57454250*) return 0;;
    00000100*) return 0;;
    ffd8ff*) return 0;;
  esac
  head -c 2048 "$1" | grep -qi '<svg'
}
emit() {
  [ -f "$2" ] || return 0
  [ -L "$2" ] && return 0
  case $1 in
    E) printf 'EXISTS %s\n' "$2";;
    R) s=$(size_of "$2")
       if [ "$s" -le "$CAP" ]; then printf 'FILE R %s %s\n' "$s" "$2"; b64 "$2"; printf '\n'; fi;;
    I) s=$(size_of "$2")
       if [ "$s" -gt 0 ] && [ "$s" -le "$CAP" ] && magic_ok "$2"; then
         if [ $((used + s)) -le "$BUDGET" ]; then
           used=$((used + s)); printf 'FILE I %s %s\n' "$s" "$2"; b64 "$2"; printf '\n'
         else
           printf 'SKIP I %s %s\n' "$s" "$2"
         fi
       fi;;
  esac
}
IFS=$NL
set -f
"#;

fn prelude() -> String {
    PRELUDE
        .replace("@CAP@", &MAX_ICON_BYTES.to_string())
        .replace("@BUDGET@", &BUDGET_BYTES.to_string())
}

fn scan_script() -> String {
    let list: Vec<String> = probe_list()
        .into_iter()
        .map(|(probe, path)| format!("{} {path}", probe.tag()))
        .collect();
    format!(
        r#"{prelude}url=$(git config --get remote.origin.url 2>/dev/null)
[ -n "$url" ] && printf 'REMOTE %s\n' "$url"
LIST='{list}'
pkgs=.
n=0
set +f
for d in apps/*/ packages/*/; do
  [ -d "$d" ] || continue
  [ -L "${{d%/}}" ] && continue
  n=$((n + 1))
  [ "$n" -gt {MAX_PACKAGES} ] && break
  pkgs="$pkgs$NL${{d%/}}"
done
set -f
for p in $pkgs; do
  printf 'PKG %s\n' "$p"
  for rec in $LIST; do
    k=${{rec%% *}}; f=${{rec#* }}
    if [ "$p" = . ]; then full=$f; else full=$p/$f; fi
    emit "$k" "$full"
  done
done
"#,
        list = list.join("\n"),
        prelude = prelude(),
    )
}

fn fetch_script(paths: &[String]) -> String {
    format!(
        "{}LIST='{}'\nfor f in $LIST; do emit I \"$f\"; done\n",
        prelude(),
        paths.join("\n")
    )
}

fn command(script: String, root: &str) -> CommandSpec {
    CommandSpec::new("sh").args(["-c", script.as_str(), "sh", root])
}

// ----- reading what the commands print -----------------------------------------------------

/// What one probed path turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Found {
    Exists,
    Text(String),
    Image(Vec<u8>),
    /// A valid image the budget left out.
    Unsent,
}

/// What the commands reported about a project.
#[derive(Debug, Default)]
struct Scan {
    remote: Option<String>,
    packages: Vec<String>,
    found: HashMap<String, Found>,
    /// Paths a command looked at, found or not.
    probed: HashSet<String>,
}

impl Scan {
    fn has(&self, path: &str) -> bool {
        self.found.contains_key(path)
    }

    fn text(&self, path: &str) -> Option<&str> {
        match self.found.get(path) {
            Some(Found::Text(text)) => Some(text),
            _ => None,
        }
    }
}

fn parse_output(output: &str, scan: &mut Scan) -> bool {
    let mut lines = output.lines().map(|line| line.trim_end_matches('\r'));
    // A login banner may come first.
    if !lines.by_ref().any(|line| line == "LEON-ICON 1") {
        return false;
    }
    while let Some(line) = lines.next() {
        let mut words = line.splitn(4, ' ');
        match words.next() {
            Some("REMOTE") => scan.remote = line.strip_prefix("REMOTE ").map(str::to_owned),
            Some("PKG") => {
                if let Some(package) = line.strip_prefix("PKG ") {
                    scan.packages.push(package.to_owned());
                }
            }
            Some("EXISTS") => {
                if let Some(path) = line.strip_prefix("EXISTS ") {
                    scan.found.insert(path.to_owned(), Found::Exists);
                }
            }
            Some("SKIP") => {
                let path = words.nth(2).unwrap_or_default();
                scan.found.insert(path.to_owned(), Found::Unsent);
            }
            Some("FILE") => {
                let (kind, _size, path) = (
                    words.next().unwrap_or_default(),
                    words.next(),
                    words.next().unwrap_or_default(),
                );
                let payload = lines.next().unwrap_or_default();
                let Some(bytes) = base64_decode(payload) else {
                    continue;
                };
                let found = if kind == "R" {
                    Found::Text(String::from_utf8_lossy(&bytes).into_owned())
                } else {
                    Found::Image(bytes)
                };
                scan.found.insert(path.to_owned(), found);
            }
            _ => {}
        }
    }
    true
}

/// Standard base64, whitespace ignored; `None` for anything else.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ----- reading the files the rules point at -------------------------------------------------

/// The `href`s of `<link rel="icon">` tags and `{ rel: 'icon', href }`
/// objects in `source`, in order of preference: plain icons before
/// `apple-touch-icon`s.
pub fn icon_links(source: &str) -> Vec<String> {
    let mut plain = Vec::new();
    let mut touch = Vec::new();
    let mut push = |rel: &str, href: &str| {
        let rel = rel.to_lowercase();
        if rel.contains("apple-touch-icon") {
            touch.push(href.to_owned());
        } else if rel.split_whitespace().any(|word| word == "icon") {
            plain.push(href.to_owned());
        }
    };
    // Tags: `<link ... rel="icon" ... href="...">`, in either order.
    let mut rest = source;
    while let Some(at) = rest.find("<link") {
        rest = &rest[at + 5..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        if let (Some(rel), Some(href)) = (attribute(tag, "rel"), attribute(tag, "href")) {
            push(&rel, &href);
        }
    }
    // Objects: `{ rel: 'icon', href: '/x.png' }`.
    let mut rest = source;
    while let Some(at) = rest.find("rel") {
        let after = &rest[at + 3..];
        rest = after;
        let Some(rel) = quoted_after(after, ':') else {
            continue;
        };
        let open = source[..source.len() - after.len()].rfind('{').unwrap_or(0);
        let close = after.find('}').unwrap_or(after.len());
        let object = &source[open..source.len() - after.len() + close];
        if let Some(href) = object
            .find("href")
            .and_then(|h| quoted_after(&object[h + 4..], ':'))
        {
            push(&rel, &href);
        }
    }
    plain.extend(touch);
    plain
}

/// The value of `name="..."` (or `name='...'`, or `name={"..."}`) in a tag.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(found) = tag[from..].find(name) {
        let start = from + found;
        from = start + name.len();
        let before_ok = tag[..start].chars().last().is_none_or(char::is_whitespace);
        if before_ok {
            if let Some(value) = quoted_after(&tag[from..], '=') {
                return Some(value);
            }
        }
    }
    None
}

/// The quoted string after `sep`, allowing spaces and a `{` before it.
fn quoted_after(text: &str, sep: char) -> Option<String> {
    let text = text.trim_start();
    let text = text.strip_prefix(sep)?.trim_start();
    let text = text.strip_prefix('{').map_or(text, str::trim_start);
    let quote = text
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let inner = &text[1..];
    inner.find(quote).map(|end| inner[..end].to_owned())
}

/// The largest `src` of a web manifest's `icons`.
pub fn manifest_icons(json: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let side = |sizes: &str| {
        sizes
            .split_whitespace()
            .filter_map(|size| size.split('x').next()?.parse::<u32>().ok())
            .max()
            .unwrap_or(if sizes.contains("any") { 1024 } else { 0 })
    };
    let mut icons: Vec<(u32, String)> = value["icons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|icon| {
            let src = icon["src"].as_str()?.to_owned();
            Some((side(icon["sizes"].as_str().unwrap_or("")), src))
        })
        .collect();
    icons.sort_by_key(|(side, _)| std::cmp::Reverse(*side));
    icons.into_iter().map(|(_, src)| src).collect()
}

/// The icon an Expo config names: `expo.icon` of `app.json`, or an
/// `icon: '...'` in a JavaScript config.
pub fn expo_icon(source: &str) -> Vec<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(source) {
        return [&value["expo"]["icon"], &value["icon"]]
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
    }
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("icon") {
        rest = &rest[at + 4..];
        if let Some(value) = quoted_after(rest, ':') {
            found.push(value);
        }
    }
    found
}

/// The `favicon` and `logo` of a Mintlify config.
pub fn mintlify_icons(json: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut take = |v: &serde_json::Value| {
        if let Some(text) = v.as_str() {
            found.push(text.to_owned());
        }
    };
    take(&value["favicon"]);
    take(&value["logo"]);
    take(&value["logo"]["light"]);
    take(&value["logo"]["dark"]);
    found
}

fn dependencies(package_json: &str) -> HashSet<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(package_json) else {
        return HashSet::new();
    };
    [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ]
    .into_iter()
    .filter_map(|section| value[section].as_object())
    .flat_map(|section| section.keys().cloned())
    .collect()
}

// ----- deciding ---------------------------------------------------------------------------

/// One file that may be the icon, and the rule that named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The rule's id.
    pub rule: &'static str,
    /// The path from the project root.
    pub path: String,
}

fn holds(marker: &Marker, package: &str, scan: &Scan, deps: &HashSet<String>) -> bool {
    match marker {
        Marker::Dep(name) => deps.contains(*name),
        Marker::File(file) => scan.has(&in_package(package, file)),
        Marker::All(all) => all.iter().all(|m| holds(m, package, scan, deps)),
        Marker::Any(any) => any.iter().any(|m| holds(m, package, scan, deps)),
        Marker::Always => true,
    }
}

/// The rule of a package: the first framework rule that holds, else the
/// generic one.
fn rule_of(package: &str, scan: &Scan) -> &'static IconRule {
    let deps = scan
        .text(&in_package(package, "package.json"))
        .map(dependencies)
        .unwrap_or_default();
    RULES
        .iter()
        .find(|rule| holds(&rule.marker, package, scan, &deps))
        .unwrap_or(&GENERIC)
}

fn declared_paths(rule: &IconRule, package: &str, scan: &Scan) -> Vec<String> {
    let mut out = Vec::new();
    for declared in rule.declared {
        for file in declared.files() {
            let full = in_package(package, file);
            let Some(text) = scan.text(&full) else {
                continue;
            };
            let dir = full.rsplit_once('/').map_or("", |(dir, _)| dir);
            let references = match declared {
                Declared::Links(_) => icon_links(text),
                Declared::Manifest(_) => manifest_icons(text),
                Declared::Expo(_) => expo_icon(text),
                Declared::Mintlify(_) => mintlify_icons(text),
            };
            for reference in references {
                out.extend(resolve(package, dir, &reference));
            }
        }
    }
    out
}

/// Where a reference found in a file of `dir` may point, inside `package`.
/// Absolute URLs and anything that leaves the package give nothing.
pub fn resolve(package: &str, dir: &str, reference: &str) -> Vec<String> {
    let reference = reference.split(['?', '#']).next().unwrap_or("").trim();
    if reference.is_empty()
        || reference.split('/').any(|part| part == "..")
        || reference.contains("://")
        || reference.starts_with("//")
        || reference.starts_with("data:")
        || !has_icon_ext(reference)
    {
        return Vec::new();
    }
    let roots: Vec<String> = if let Some(absolute) = reference.strip_prefix('/') {
        ["public", "static", ""]
            .iter()
            .filter_map(|root| join_within(package, &in_package(package, root), absolute))
            .collect()
    } else {
        std::iter::once(join_within(package, dir, reference))
            .chain(
                ["public", "static", ""]
                    .iter()
                    .map(|root| join_within(package, &in_package(package, root), reference)),
            )
            .flatten()
            .collect()
    };
    let mut seen = HashSet::new();
    roots
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// Every file that may be the icon, best first: the frameworks of the
/// packages that declare one, root first, then the generic list everywhere.
fn candidates(scan: &Scan) -> Vec<Candidate> {
    let mut packages: Vec<&str> = scan.packages.iter().map(String::as_str).collect();
    if packages.is_empty() {
        packages.push(".");
    }
    let mut out: Vec<Candidate> = Vec::new();
    let mut add = |rule: &'static IconRule, rule_files: Vec<String>| {
        for path in rule_files {
            if safe_path(&path) && !out.iter().any(|c| c.path == path) {
                out.push(Candidate {
                    rule: rule.id,
                    path,
                });
            }
        }
    };
    let statics = |rule: &IconRule, package: &str| -> Vec<String> {
        rule.candidates
            .iter()
            .flat_map(|pattern| expand(pattern))
            .map(|file| in_package(package, &file))
            .collect()
    };
    for package in &packages {
        let rule = rule_of(package, scan);
        if std::ptr::eq(rule, &GENERIC) {
            continue;
        }
        let declared = declared_paths(rule, package, scan);
        let files = statics(rule, package);
        let ordered = if rule.declared_first {
            [declared, files].concat()
        } else {
            [files, declared].concat()
        };
        add(rule, ordered);
    }
    for package in &packages {
        let mut files = statics(&GENERIC, package);
        files.extend(declared_paths(&GENERIC, package, scan));
        add(&GENERIC, files);
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Found(Candidate, IconImage),
    /// These candidates were not seen; ask for them.
    Fetch(Vec<String>),
    Nothing,
}

fn decide(scan: &Scan, candidates: &[Candidate], may_fetch: bool) -> Decision {
    for (at, candidate) in candidates.iter().enumerate() {
        let unseen = match scan.found.get(&candidate.path) {
            Some(Found::Image(bytes)) => {
                if let Some(format) = sniff(bytes) {
                    return Decision::Found(
                        candidate.clone(),
                        IconImage {
                            format,
                            bytes: bytes.clone(),
                        },
                    );
                }
                false
            }
            Some(Found::Unsent) => true,
            Some(Found::Exists | Found::Text(_)) => false,
            None => !scan.probed.contains(&candidate.path),
        };
        if unseen && may_fetch {
            // Only a second look can tell: ask for this one and the next few
            // that were not seen, in order.
            let wanted: Vec<String> = candidates[at..]
                .iter()
                .filter(|c| match scan.found.get(&c.path) {
                    Some(Found::Image(_) | Found::Exists | Found::Text(_)) => false,
                    Some(Found::Unsent) => true,
                    None => !scan.probed.contains(&c.path),
                })
                .take(12)
                .map(|c| c.path.clone())
                .collect();
            return Decision::Fetch(wanted);
        }
    }
    Decision::Nothing
}

// ----- the origin remote -------------------------------------------------------------------

/// A repository as its origin remote names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepo {
    /// The Git host, lower case.
    pub host: String,
    /// The owner: a user or an organisation.
    pub owner: String,
    /// The repository's name, without `.git`.
    pub repo: String,
}

impl RemoteRepo {
    /// `owner/repo`.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// `host/owner/repo`: how the store keeps it.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.repo)
    }

    /// Where the owner's avatar is, when the host is GitHub or a GitHub
    /// Enterprise host; `None` for any other host (nothing is sent to it).
    pub fn avatar_url(&self) -> Option<String> {
        is_github_host(&self.host)
            .then(|| format!("https://{}/{}.png?size=64", self.host, self.owner))
    }
}

/// Whether `host` is github.com or looks like a GitHub Enterprise host:
/// `*.ghe.com`, or a name whose first label is `github` or `ghe`.
pub fn is_github_host(host: &str) -> bool {
    let host = host.to_lowercase();
    let first = host.split('.').next().unwrap_or("");
    host == "github.com" || host.ends_with(".ghe.com") || first == "github" || first == "ghe"
}

/// Parses the URL forms git remotes take: `https://host/owner/repo(.git)`,
/// `ssh://git@host(:port)/owner/repo`, `git@host:owner/repo`. The ssh-over-443
/// host `ssh.github.com` is github.com.
pub fn parse_remote(url: &str) -> Option<RemoteRepo> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let authority = authority.rsplit('@').next()?;
        (authority.split(':').next()?, path)
    } else {
        let (authority, path) = url.split_once(':')?;
        (authority.rsplit('@').next()?, path)
    };
    let mut parts = path.trim_matches('/').split('/');
    let owner = parts.next().filter(|p| !p.is_empty())?;
    let repo = parts.next().filter(|p| !p.is_empty())?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let host = host.to_lowercase();
    let host = if host == "ssh.github.com" {
        "github.com".to_owned()
    } else {
        host
    };
    (!host.is_empty() && !host.contains(' ') && !repo.is_empty()).then(|| RemoteRepo {
        host,
        owner: owner.to_owned(),
        repo: repo.to_owned(),
    })
}

// ----- the entry point ---------------------------------------------------------------------

/// The logo found in a repository's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundIcon {
    /// The rule that matched.
    pub rule: String,
    /// The file, from the project root.
    pub path: String,
    /// The image.
    pub image: IconImage,
}

impl FoundIcon {
    /// What the store records as the source: `rule:path`.
    pub fn source(&self) -> String {
        format!("{}:{}", self.rule, self.path)
    }
}

/// What detection found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Detection {
    /// An image file of the repository, when there is one.
    pub icon: Option<FoundIcon>,
    /// The origin remote, when it parses.
    pub remote: Option<RemoteRepo>,
    /// How many commands ran: one, or two.
    pub round_trips: u8,
}

/// Why detection could not tell.
#[derive(Debug, Error)]
pub enum IconError {
    /// The command could not be run.
    #[error("{0}")]
    Run(#[from] RunError),
    /// The command ran and failed or printed something else.
    #[error("the icon scan failed{}", .0.as_deref().map(|s| format!(": {s}")).unwrap_or_default())]
    Failed(Option<String>),
}

/// Looks for the logo of the project rooted at `root` on `machine`.
pub async fn detect<R: Runner>(
    runner: &R,
    machine: &Machine,
    ssh: &SshOptions,
    root: &str,
) -> Result<Detection, IconError> {
    let run = |script: String| async move {
        let output = runner
            .run(&run_on(machine, &command(script, root), ssh))
            .await?;
        if !output.success() {
            let why = output.stderr.trim();
            return Err(IconError::Failed((!why.is_empty()).then(|| why.to_owned())));
        }
        Ok(output.stdout)
    };

    let mut scan = Scan::default();
    if !parse_output(&run(scan_script()).await?, &mut scan) {
        return Err(IconError::Failed(None));
    }
    // Everything of the probe list in every package was looked at.
    let list = probe_list();
    let packages: Vec<String> = if scan.packages.is_empty() {
        vec![".".to_owned()]
    } else {
        scan.packages.clone()
    };
    for package in &packages {
        for (_, path) in &list {
            scan.probed.insert(in_package(package, path));
        }
    }
    let remote = scan.remote.as_deref().and_then(parse_remote);
    let wanted = candidates(&scan);
    let mut round_trips = 1;
    let mut decision = decide(&scan, &wanted, true);
    if let Decision::Fetch(paths) = &decision {
        let paths: Vec<String> = paths.iter().filter(|p| safe_path(p)).cloned().collect();
        round_trips = 2;
        let mut extra = Scan::default();
        if parse_output(&run(fetch_script(&paths)).await?, &mut extra) {
            for path in &paths {
                scan.probed.insert(path.clone());
                scan.found.remove(path);
            }
            scan.found.extend(extra.found);
        }
        decision = decide(&scan, &wanted, false);
    }
    let icon = match decision {
        Decision::Found(candidate, image) => Some(FoundIcon {
            rule: candidate.rule.to_owned(),
            path: candidate.path,
            image,
        }),
        Decision::Fetch(_) | Decision::Nothing => None,
    };
    Ok(Detection {
        icon,
        remote,
        round_trips,
    })
}

#[cfg(test)]
mod tests;
