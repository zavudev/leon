//! Themes from files: parsing, inheritance and derivation.
//!
//! A theme file is TOML (the format is documented in `docs/THEMES.md`, and its
//! keys are generated from [`super::tokens::TOKENS`]):
//!
//! ```toml
//! id = "midnight"
//! name = "Midnight"
//! author = "Ada"
//! extends = "leon"       # a built-in or another user theme
//!
//! [dark]
//! accent = "#7AA2FF"     # accent-derived tokens follow
//! ```
//!
//! [`parse`] reads one file into a [`Raw`] without judging it; [`resolve_all`]
//! turns every file of the folder into a [`Loaded`] theme: it finds each file's
//! parent (cycles and missing parents are errors), lays the file's keys over
//! the parent's theme, recomputes what the built-in themes derive from the
//! accent when the file set only that, and checks the result against the rules
//! of [`super::check`]. Nothing here panics on bad input: whatever is wrong
//! becomes a [`Problem`].

use super::check::{check, Problem};
use super::registry::{intern, RESERVED};
use super::tokens::{self, Kind, Section, Value};
use super::{Appearance, Theme, ThemeId};
use gpui_kit::{Hsla, Rgba};
use leon_term::colors::mix;
use std::collections::{HashMap, HashSet};
use toml::{Table, Value as Toml};

/// What the loader asks of the environment.
pub struct Env<'a> {
    /// Whether a font family can be drawn: bundled with Leon or installed.
    pub has_font: &'a dyn Fn(&str) -> bool,
}

/// One file as it was written.
#[derive(Debug, Default)]
pub struct Raw {
    /// The file's name, for the reports.
    pub file: String,
    /// `id`.
    pub id: Option<String>,
    /// `name`.
    pub name: Option<String>,
    /// `author`.
    pub author: Option<String>,
    /// `extends`.
    pub extends: Option<String>,
    dark: Table,
    light: Table,
    fonts: Table,
    shape: Table,
    lines: Table,
    terminal: Table,
    /// What was wrong while reading it.
    pub problems: Vec<Problem>,
}

/// One theme of the folder, loaded or not.
#[derive(Debug)]
pub struct Loaded {
    /// The file it came from.
    pub file: String,
    /// Its id (the file's stem when the file has none that can be used).
    pub id: String,
    /// The name shown.
    pub name: String,
    /// Who made it.
    pub author: Option<String>,
    /// The theme, when the file has no error.
    pub theme: Option<Theme>,
    /// Errors and warnings.
    pub problems: Vec<Problem>,
}

impl Loaded {
    /// Whether the theme can be used.
    pub fn is_valid(&self) -> bool {
        self.theme.is_some()
    }
}

/// The edit distance between two words.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let held = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            previous = held;
        }
    }
    row[b.len()]
}

/// The known word nearest to `word`, when it is near enough to be a typo.
pub fn suggestion<'a>(word: &str, known: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (word.len() / 3).clamp(1, 3);
    known
        .into_iter()
        .map(|candidate| (distance(word, candidate), candidate))
        .filter(|(far, _)| *far <= limit)
        .min_by_key(|(far, _)| *far)
        .map(|(_, candidate)| candidate)
}

fn unknown(file: &str, key: &str, word: &str, known: &[&str], out: &mut Vec<Problem>) {
    let hint = suggestion(word, known.iter().copied())
        .map(|near| format!("; did you mean `{near}`?"))
        .unwrap_or_default();
    out.push(Problem::warning(
        file,
        key,
        format!("unknown key `{word}`, nothing reads it{hint}"),
    ));
}

fn table_of(file: &str, key: &str, value: Option<&Toml>, out: &mut Vec<Problem>) -> Table {
    match value {
        None => Table::new(),
        Some(Toml::Table(table)) => table.clone(),
        Some(_) => {
            out.push(Problem::error(file, key, "this must be a table"));
            Table::new()
        }
    }
}

fn text_of(file: &str, key: &str, value: Option<&Toml>, out: &mut Vec<Problem>) -> Option<String> {
    match value {
        None => None,
        Some(Toml::String(text)) => Some(text.trim().to_owned()),
        Some(_) => {
            out.push(Problem::error(file, key, "this must be text"));
            None
        }
    }
}

/// Reads one file. Whatever is wrong with it is in [`Raw::problems`].
pub fn parse(file: &str, source: &str) -> Raw {
    let mut raw = Raw {
        file: file.to_owned(),
        ..Raw::default()
    };
    let mut table = match source.parse::<Table>() {
        Ok(table) => table,
        Err(error) => {
            raw.problems.push(Problem::error(
                file,
                "",
                format!(
                    "this is not valid TOML: {}",
                    error.to_string().replace('\n', " ")
                ),
            ));
            return raw;
        }
    };
    raw.id = text_of(file, "id", table.remove("id").as_ref(), &mut raw.problems);
    raw.name = text_of(
        file,
        "name",
        table.remove("name").as_ref(),
        &mut raw.problems,
    );
    raw.author = text_of(
        file,
        "author",
        table.remove("author").as_ref(),
        &mut raw.problems,
    );
    raw.extends = text_of(
        file,
        "extends",
        table.remove("extends").as_ref(),
        &mut raw.problems,
    );
    table.remove("description");
    raw.dark = table_of(
        file,
        "dark",
        table.remove("dark").as_ref(),
        &mut raw.problems,
    );
    raw.light = table_of(
        file,
        "light",
        table.remove("light").as_ref(),
        &mut raw.problems,
    );
    raw.fonts = table_of(
        file,
        "fonts",
        table.remove("fonts").as_ref(),
        &mut raw.problems,
    );
    raw.shape = table_of(
        file,
        "shape",
        table.remove("shape").as_ref(),
        &mut raw.problems,
    );
    raw.lines = table_of(
        file,
        "lines",
        table.remove("lines").as_ref(),
        &mut raw.problems,
    );
    raw.terminal = table_of(
        file,
        "terminal",
        table.remove("terminal").as_ref(),
        &mut raw.problems,
    );
    let top = [
        "id",
        "name",
        "author",
        "extends",
        "description",
        "dark",
        "light",
        "fonts",
        "shape",
        "lines",
        "terminal",
    ];
    for word in table.keys() {
        unknown(file, word, word, &top, &mut raw.problems);
    }
    raw
}

fn keys_of(section: Section) -> Vec<&'static str> {
    tokens::TOKENS
        .iter()
        .filter(|token| token.section == section)
        .map(|token| token.key)
        .collect()
}

/// What a file set: the keys that were given, per appearance and for the
/// shared tables, so that what was not given can be derived.
#[derive(Default)]
struct Given {
    keys: HashSet<(Option<Appearance>, Section, &'static str)>,
}

impl Given {
    fn has(&self, appearance: Appearance, section: Section, key: &str) -> bool {
        self.keys
            .iter()
            .any(|(a, s, k)| *a == Some(appearance) && *s == section && *k == key)
    }
}

fn read_value(
    file: &str,
    path: &str,
    token: &tokens::Token,
    value: &Toml,
    out: &mut Vec<Problem>,
) -> Option<Value> {
    match (token.kind, value) {
        (Kind::Colour { alpha }, Toml::String(text)) => match tokens::parse_colour(text) {
            Ok(colour) => {
                if !alpha && Rgba::from(colour).a < 0.999 {
                    out.push(Problem::error(
                        file,
                        path,
                        "this colour must be opaque: write `#RRGGBB`",
                    ));
                    None
                } else {
                    Some(Value::Colour(colour))
                }
            }
            Err(message) => {
                out.push(Problem::error(file, path, message));
                None
            }
        },
        (Kind::Colours, Toml::Array(items)) => {
            if items.len() != 16 {
                out.push(Problem::error(
                    file,
                    path,
                    format!("this needs the sixteen ANSI colours, not {}", items.len()),
                ));
                return None;
            }
            let mut colours: Vec<Hsla> = Vec::new();
            for (i, item) in items.iter().enumerate() {
                match item.as_str().map(tokens::parse_colour) {
                    Some(Ok(colour)) if Rgba::from(colour).a > 0.999 => colours.push(colour),
                    Some(Err(message)) => {
                        out.push(Problem::error(file, &format!("{path}[{i}]"), message));
                    }
                    _ => out.push(Problem::error(
                        file,
                        &format!("{path}[{i}]"),
                        "this must be an opaque `#RRGGBB` colour",
                    )),
                }
            }
            (colours.len() == 16).then_some(Value::Colours(colours))
        }
        (Kind::Number { min, max }, Toml::Float(_) | Toml::Integer(_)) => {
            let number = match value {
                Toml::Float(number) => *number as f32,
                Toml::Integer(number) => *number as f32,
                _ => unreachable!("matched above"),
            };
            if !(min..=max).contains(&number) {
                out.push(Problem::error(
                    file,
                    path,
                    format!("{number} is out of range: it must be between {min} and {max}"),
                ));
                None
            } else {
                Some(Value::Number(number))
            }
        }
        (Kind::Bool, Toml::Boolean(on)) => Some(Value::Bool(*on)),
        (Kind::Text, Toml::String(text)) if !text.trim().is_empty() => {
            Some(Value::Text(text.trim().to_owned()))
        }
        (Kind::Choice(words), Toml::String(text)) => {
            if words.contains(&text.as_str()) {
                Some(Value::Text(text.clone()))
            } else {
                let hint = suggestion(text, words.iter().copied())
                    .map(|near| format!("; did you mean `{near}`?"))
                    .unwrap_or_default();
                out.push(Problem::error(
                    file,
                    path,
                    format!("{text:?} is not one of {}{hint}", words.join(", ")),
                ));
                None
            }
        }
        (kind, _) => {
            let wanted = match kind {
                Kind::Colour { .. } => "a colour written `#RRGGBB`",
                Kind::Colours => "a list of sixteen colours",
                Kind::Number { .. } => "a number",
                Kind::Bool => "`true` or `false`",
                Kind::Text => "text",
                Kind::Choice(_) => "one of the listed words",
            };
            out.push(Problem::error(file, path, format!("this must be {wanted}")));
            None
        }
    }
}

/// Lays the keys of `table` (of `section`, for `appearance` or both) over
/// `theme`.
#[allow(clippy::too_many_arguments)]
fn lay_over(
    raw_file: &str,
    prefix: &str,
    table: &Table,
    section: Section,
    appearances: &[Appearance],
    theme: &mut Theme,
    given: &mut Given,
    env: &Env,
    out: &mut Vec<Problem>,
) {
    let known = keys_of(section);
    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        // A terminal table inside an appearance is handled by the caller.
        if section == Section::Palette && key == "terminal" {
            continue;
        }
        let Some(token) = tokens::find(section, key) else {
            unknown(raw_file, &path, key, &known, out);
            continue;
        };
        let Some(mut value) = read_value(raw_file, &path, token, value, out) else {
            continue;
        };
        if section == Section::Fonts {
            if let Value::Text(family) = &value {
                if !(env.has_font)(family) {
                    out.push(Problem::warning(
                        raw_file,
                        &path,
                        format!("the font {family:?} is neither bundled with Leon nor installed; the parent's is used"),
                    ));
                    continue;
                }
            }
        }
        if let (Section::Lines, Kind::Number { .. }, Value::Number(number)) =
            (section, token.kind, &mut value)
        {
            if token.key == "weight" && *number != 1.0 && *number != 2.0 {
                out.push(Problem::error(raw_file, &path, "the weight is 1 or 2"));
                continue;
            }
        }
        if appearances.is_empty() {
            (token.set)(theme, Appearance::Dark, value);
            given.keys.insert((None, section, token.key));
        } else {
            for appearance in appearances {
                (token.set)(theme, *appearance, value.clone());
                given.keys.insert((Some(*appearance), section, token.key));
            }
        }
    }
}

/// The share of the way from `background` to `accent` that `tinted` is.
fn tint_of(background: Hsla, accent: Hsla, tinted: Hsla) -> f32 {
    let (b, a, t) = (
        Rgba::from(background),
        Rgba::from(accent),
        Rgba::from(tinted),
    );
    let along = [a.r - b.r, a.g - b.g, a.b - b.b];
    let to = [t.r - b.r, t.g - b.g, t.b - b.b];
    let length: f32 = along.iter().map(|c| c * c).sum();
    if length < 1e-6 {
        return 0.3;
    }
    let dot: f32 = along.iter().zip(to).map(|(x, y)| x * y).sum();
    (dot / length).clamp(0.05, 0.95)
}

/// Recomputes what a theme derives from its accent and its page where the file
/// did not say.
fn derive(parent: &Theme, theme: &mut Theme, given: &Given) {
    for appearance in [Appearance::Dark, Appearance::Light] {
        let before = parent.palette(appearance);
        let set = |section, key| given.has(appearance, section, key);
        let accent_set = set(Section::Palette, "accent");
        let page_set = set(Section::Palette, "background");
        let text_set = set(Section::Palette, "text");
        let p = theme.palette_mut(appearance);
        if accent_set {
            if before.accent_fill == before.signal && !set(Section::Palette, "accent_fill") {
                p.accent_fill = p.signal;
            }
            if before.primary_fill == before.signal && !set(Section::Palette, "primary_fill") {
                p.primary_fill = p.signal;
            }
            if before.logo == before.signal && !set(Section::Palette, "logo") {
                p.logo = p.signal;
            }
            if before.terminal.cursor == before.signal && !set(Section::Terminal, "cursor") {
                p.terminal.cursor = p.signal;
            }
            if before.terminal.find_match_current == before.signal
                && !set(Section::Terminal, "find_match_current")
            {
                p.terminal.find_match_current = p.signal;
            }
            // What is drawn on a fill is the most legible of the candidates.
            let best = |fill: Hsla, current: Hsla, page: Hsla, text: Hsla| {
                [current, page, text]
                    .into_iter()
                    .max_by(|a, b| {
                        super::check::contrast(*a, fill)
                            .total_cmp(&super::check::contrast(*b, fill))
                    })
                    .unwrap_or(current)
            };
            if !set(Section::Palette, "on_accent_fill") {
                p.on_accent_fill = best(p.accent_fill, p.on_accent_fill, p.background, p.text);
            }
            if !set(Section::Palette, "on_primary") {
                p.on_primary = best(p.primary_fill, p.on_primary, p.background, p.text);
            }
        }
        if page_set
            && before.terminal.background == before.background
            && !set(Section::Terminal, "background")
        {
            p.terminal.background = p.background;
        }
        if text_set
            && before.terminal.foreground == before.text
            && !set(Section::Terminal, "foreground")
        {
            p.terminal.foreground = p.text;
        }
        if accent_set || page_set {
            if !set(Section::Terminal, "selection") {
                let tint = tint_of(before.background, before.signal, before.terminal.selection);
                p.terminal.selection = mix(p.background, p.signal, tint);
            }
            if !set(Section::Terminal, "find_match") {
                let tint = tint_of(before.background, before.signal, before.terminal.find_match);
                p.terminal.find_match = mix(p.background, p.signal, tint);
            }
        }
    }
}

/// Every token a file with no parent must set (the others are derived).
fn required() -> Vec<(Option<Appearance>, Section, &'static str)> {
    let mut all = Vec::new();
    for token in tokens::TOKENS {
        let derived_terminal = token.section == Section::Terminal
            && matches!(
                token.key,
                "foreground"
                    | "background"
                    | "cursor"
                    | "selection"
                    | "find_match"
                    | "find_match_current"
            );
        if derived_terminal {
            continue;
        }
        if token.section.per_appearance() {
            for appearance in [Appearance::Dark, Appearance::Light] {
                all.push((Some(appearance), token.section, token.key));
            }
        } else {
            all.push((None, token.section, token.key));
        }
    }
    all
}

/// A standalone theme's derived terminal colours, from its palette.
fn standalone_terminal(theme: &mut Theme, given: &Given) {
    for appearance in [Appearance::Dark, Appearance::Light] {
        let set = |key| given.has(appearance, Section::Terminal, key);
        let p = theme.palette_mut(appearance);
        if !set("foreground") {
            p.terminal.foreground = p.text;
        }
        if !set("background") {
            p.terminal.background = p.background;
        }
        if !set("cursor") {
            p.terminal.cursor = p.signal;
        }
        if !set("selection") {
            p.terminal.selection = mix(p.background, p.signal, 0.3);
        }
        if !set("find_match") {
            p.terminal.find_match = mix(p.background, p.signal, 0.2);
        }
        if !set("find_match_current") {
            p.terminal.find_match_current = p.signal;
        }
    }
}

/// The key of a file for a theme: the lower case words of its id.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if matches!(c, '-' | '_' | ' ' | '.') && !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// Whether `id` is a usable id: lower case letters, digits and dashes.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// Turns the files of a folder into themes: parents, derivation, rules.
pub fn resolve_all(raws: Vec<Raw>, env: &Env) -> Vec<Loaded> {
    // Which file stands for which id.
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut loaded: Vec<Option<Loaded>> = Vec::new();
    let mut fatal: Vec<Option<Problem>> = Vec::new();
    for (index, raw) in raws.iter().enumerate() {
        let stem = raw.file.trim_end_matches(".toml").to_owned();
        let mut problem = raw.problems.iter().find(|p| p.is_error()).cloned();
        let id = match &raw.id {
            Some(id) if valid_id(id) => id.clone(),
            Some(id) => {
                problem.get_or_insert_with(|| {
                    Problem::error(
                        &raw.file,
                        "id",
                        format!("{id:?} is not an id: use lower case letters, digits and dashes"),
                    )
                });
                slug(&stem)
            }
            None => {
                problem
                    .get_or_insert_with(|| Problem::error(&raw.file, "id", "the file has no `id`"));
                slug(&stem)
            }
        };
        if RESERVED.contains(&id.as_str()) && raw.id.is_some() {
            problem = Some(Problem::error(
                &raw.file,
                "id",
                format!("`{id}` is a built-in theme's id; a file cannot take it: choose another"),
            ));
        } else if let Some(first) = by_id.get(&id) {
            problem = Some(Problem::error(
                &raw.file,
                "id",
                format!("`{id}` is already the id of {}", raws[*first].file),
            ));
        } else if problem.is_none() || !RESERVED.contains(&id.as_str()) {
            by_id.insert(id.clone(), index);
        }
        let id = if RESERVED.contains(&id.as_str()) {
            format!("file-{}", slug(&stem))
        } else {
            id
        };
        loaded.push(Some(Loaded {
            file: raw.file.clone(),
            id,
            name: raw
                .name
                .clone()
                .unwrap_or_else(|| raw.id.clone().unwrap_or(stem)),
            author: raw.author.clone(),
            theme: None,
            problems: raw.problems.clone(),
        }));
        fatal.push(problem);
    }
    let mut done: HashMap<usize, Result<Theme, ()>> = HashMap::new();
    let mut results: Vec<Loaded> = Vec::new();
    for index in 0..raws.len() {
        let mut stack = Vec::new();
        let outcome = resolve_one(
            index,
            &raws,
            &by_id,
            &fatal,
            env,
            &mut done,
            &mut stack,
            &mut loaded,
        );
        let mut entry = loaded[index].take().expect("each file is resolved once");
        if let Ok(theme) = outcome {
            entry.theme = Some(theme);
        }
        if let Some(problem) = &fatal[index] {
            if !entry.problems.contains(problem) {
                entry.problems.push(problem.clone());
            }
            entry.theme = None;
        }
        if entry.problems.iter().any(Problem::is_error) {
            entry.theme = None;
        }
        results.push(entry);
    }
    results
}

#[allow(clippy::too_many_arguments)]
fn resolve_one(
    index: usize,
    raws: &[Raw],
    by_id: &HashMap<String, usize>,
    fatal: &[Option<Problem>],
    env: &Env,
    done: &mut HashMap<usize, Result<Theme, ()>>,
    stack: &mut Vec<usize>,
    loaded: &mut [Option<Loaded>],
) -> Result<Theme, ()> {
    if let Some(known) = done.get(&index) {
        return *known;
    }
    let raw = &raws[index];
    let mut problems: Vec<Problem> = Vec::new();
    let fail = |problems: &mut Vec<Problem>, problem: Problem| {
        problems.push(problem);
    };
    if fatal[index].is_some() {
        done.insert(index, Err(()));
        return Err(());
    }
    stack.push(index);
    // The parent.
    let parent: Option<Theme> = match &raw.extends {
        None => None,
        Some(wanted) => {
            let wanted = slug(wanted);
            if let Some(builtin) = ThemeId::ALL.iter().find(|id| id.slug() == wanted) {
                Some(builtin.theme())
            } else if let Some(&at) = by_id.get(&wanted) {
                if stack.contains(&at) {
                    let path: Vec<String> = stack
                        .iter()
                        .skip_while(|member| **member != at)
                        .map(|member| raws[*member].id.clone().unwrap_or_default())
                        .chain([wanted.clone()])
                        .collect();
                    fail(
                        &mut problems,
                        Problem::error(
                            &raw.file,
                            "extends",
                            format!("a theme cannot extend itself: {}", path.join(" -> ")),
                        ),
                    );
                    None
                } else {
                    match resolve_one(at, raws, by_id, fatal, env, done, stack, loaded) {
                        Ok(theme) => Some(theme),
                        Err(()) => {
                            fail(
                                &mut problems,
                                Problem::error(
                                    &raw.file,
                                    "extends",
                                    format!("the parent `{wanted}` has errors of its own"),
                                ),
                            );
                            None
                        }
                    }
                }
            } else {
                let known: Vec<&str> = ThemeId::ALL
                    .iter()
                    .map(|id| id.slug())
                    .chain(by_id.keys().map(String::as_str))
                    .collect();
                let hint = suggestion(&wanted, known.iter().copied())
                    .map(|near| format!("; did you mean `{near}`?"))
                    .unwrap_or_default();
                fail(
                    &mut problems,
                    Problem::error(
                        &raw.file,
                        "extends",
                        format!("there is no theme `{wanted}` to extend{hint}"),
                    ),
                );
                None
            }
        }
    };
    stack.pop();
    let broken = raw.extends.is_some() && parent.is_none();
    let mut theme = parent.unwrap_or_else(|| ThemeId::DEFAULT.theme());
    let before = theme;
    let mut given = Given::default();
    if !broken {
        let both = [Appearance::Dark, Appearance::Light];
        // The shared tables first, then each appearance's own.
        lay_over(
            &raw.file,
            "terminal",
            &raw.terminal,
            Section::Terminal,
            &both,
            &mut theme,
            &mut given,
            env,
            &mut problems,
        );
        for (appearance, table, prefix) in [
            (Appearance::Dark, &raw.dark, "dark"),
            (Appearance::Light, &raw.light, "light"),
        ] {
            lay_over(
                &raw.file,
                prefix,
                table,
                Section::Palette,
                &[appearance],
                &mut theme,
                &mut given,
                env,
                &mut problems,
            );
            let nested = match table.get("terminal") {
                Some(Toml::Table(nested)) => nested.clone(),
                Some(_) => {
                    problems.push(Problem::error(
                        &raw.file,
                        &format!("{prefix}.terminal"),
                        "this must be a table",
                    ));
                    Table::new()
                }
                None => Table::new(),
            };
            lay_over(
                &raw.file,
                &format!("{prefix}.terminal"),
                &nested,
                Section::Terminal,
                &[appearance],
                &mut theme,
                &mut given,
                env,
                &mut problems,
            );
        }
        lay_over(
            &raw.file,
            "fonts",
            &raw.fonts,
            Section::Fonts,
            &[],
            &mut theme,
            &mut given,
            env,
            &mut problems,
        );
        lay_over(
            &raw.file,
            "shape",
            &raw.shape,
            Section::Shape,
            &[],
            &mut theme,
            &mut given,
            env,
            &mut problems,
        );
        lay_over(
            &raw.file,
            "lines",
            &raw.lines,
            Section::Lines,
            &[],
            &mut theme,
            &mut given,
            env,
            &mut problems,
        );
        if raw.extends.is_some() {
            derive(&before, &mut theme, &given);
        } else {
            // A standalone theme has to say everything but the derived
            // terminal colours.
            let missing: Vec<String> = required()
                .into_iter()
                .filter(|(appearance, section, key)| match appearance {
                    Some(a) => !given.has(*a, *section, key),
                    None => !given.keys.iter().any(|(_, s, k)| s == section && k == key),
                })
                .map(|(appearance, section, key)| {
                    let prefix = match (appearance, section) {
                        (Some(Appearance::Dark), Section::Terminal) => "dark.terminal.",
                        (Some(Appearance::Light), Section::Terminal) => "light.terminal.",
                        (Some(Appearance::Dark), _) => "dark.",
                        (Some(Appearance::Light), _) => "light.",
                        (None, Section::Fonts) => "fonts.",
                        (None, Section::Shape) => "shape.",
                        _ => "lines.",
                    };
                    format!("{prefix}{key}")
                })
                .collect();
            if !missing.is_empty() {
                problems.push(Problem::error(
                    &raw.file,
                    "",
                    format!(
                        "a theme without `extends` has to set every token; missing: {}",
                        missing.join(", ")
                    ),
                ));
            }
            standalone_terminal(&mut theme, &given);
        }
        problems.extend(check(&raw.file, &theme));
    }
    let errors = problems.iter().any(Problem::is_error);
    if let Some(entry) = loaded[index].as_mut() {
        for problem in problems {
            if !entry.problems.contains(&problem) {
                entry.problems.push(problem);
            }
        }
    }
    let result = if errors || broken { Err(()) } else { Ok(theme) };
    done.insert(index, result);
    result
}

/// The user themes of a set of files as registry entries.
pub fn entries(loaded: Vec<Loaded>, folder: &std::path::Path) -> Vec<super::registry::Entry> {
    use super::registry::{Entry, Origin};
    loaded
        .into_iter()
        .map(|item| {
            let first_error = item
                .problems
                .iter()
                .find(|p| p.is_error())
                .map(|p| p.summary());
            let detail = match (&first_error, &item.author) {
                (Some(error), _) => format!("invalid: {error}"),
                (None, Some(author)) => format!("user theme by {author}"),
                (None, None) => "user theme".to_owned(),
            };
            Entry {
                id: super::registry::id_of(&item.id),
                name: intern(&item.name),
                detail: intern(&detail),
                origin: Origin::User(folder.join(&item.file)),
                valid: item.theme.is_some(),
                theme: item.theme,
                problems: item.problems,
            }
        })
        .collect()
}
