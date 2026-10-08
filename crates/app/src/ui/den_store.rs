//! The dens the user keeps: files in a folder beside the settings.
//!
//! A den is one JSON file, `dens/<id>.json` beside `settings.json`, in the
//! format of [`leon_den::layout`] (see `docs/DEN.md`). The id is the name of
//! the file, made from the den's name; the built-in dens have ids of their
//! own that no file can take. Which den is in use is the setting `den`: the
//! id of a built-in den or of a file.
//!
//! Everything here is a function of a folder and its files, with no window:
//! the shell calls it ([`super::den_edit`]) and reports what it answers.
//! Reading never fails: a file that is no den gives the default one and a
//! note, as [`DenLayout::load`] does.
//!
//! [`DenLayout::load`]: leon_den::DenLayout::load

use std::io;
use std::path::{Path, PathBuf};

use leon_den::layout::Loaded;
use leon_den::prefabs::{default_layout, prefab, prefabs, OFFICE};
use leon_den::DenLayout;

/// The name of the folder, beside the settings file.
pub const FOLDER: &str = "dens";

/// A den that can be chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DenChoice {
    /// What the setting holds: a prefab's id, or the name of a file.
    pub id: String,
    /// Its name.
    pub name: String,
    /// A line about it.
    pub detail: String,
    /// Whether it is a file of the user's rather than a built-in den.
    pub user: bool,
}

/// The id of a den called `name`: its letters and digits in lower case,
/// joined by dashes. Empty when the name has none.
pub fn id_for(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_end_matches('-').chars().take(48).collect()
}

/// Whether a text is an id as [`id_for`] makes them: what may be the name of
/// a file here. A setting written by hand cannot point out of the folder.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id_for(id) == id
}

/// Whether an id is a built-in den's.
pub fn is_prefab(id: &str) -> bool {
    prefab(id).is_some()
}

fn file(folder: &Path, id: &str) -> PathBuf {
    folder.join(format!("{id}.json"))
}

/// An id for a new den called `name` that no built-in den and no file has:
/// the name's own, or that with a number after it. `None` when the name has
/// no letter or digit.
pub fn free_id(folder: Option<&Path>, name: &str) -> Option<String> {
    let base = id_for(name);
    if base.is_empty() {
        return None;
    }
    let taken = |id: &str| is_prefab(id) || folder.is_some_and(|folder| file(folder, id).exists());
    if !taken(&base) {
        return Some(base);
    }
    (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id))
}

/// Every den there is to choose: the built-in ones, then the user's by name.
pub fn list(folder: Option<&Path>) -> Vec<DenChoice> {
    let mut all: Vec<DenChoice> = prefabs()
        .into_iter()
        .map(|prefab| DenChoice {
            id: prefab.id.to_owned(),
            name: prefab.layout.name.clone(),
            detail: prefab.about.to_owned(),
            user: false,
        })
        .collect();
    let mut own = Vec::new();
    let entries = folder.and_then(|folder| std::fs::read_dir(folder).ok());
    for entry in entries.into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(id) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|_| path.extension().is_some_and(|ext| ext == "json"))
            .filter(|id| valid_id(id) && !is_prefab(id))
        else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let loaded = DenLayout::load(&text, &default_layout());
        let broken = loaded
            .notes
            .first()
            .is_some_and(|note| note.starts_with("This is not a den"));
        own.push(DenChoice {
            id: id.to_owned(),
            name: if broken {
                id.to_owned()
            } else {
                loaded.layout.name.clone()
            },
            detail: if broken {
                "This file is not a den.".to_owned()
            } else {
                format!(
                    "Yours \u{b7} {} by {} \u{b7} {} pieces",
                    loaded.layout.cols,
                    loaded.layout.rows,
                    loaded.layout.items.len()
                )
            },
            user: true,
        });
    }
    own.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
    all.extend(own);
    all
}

/// The den with this id, and the id it really is: a built-in den, the
/// user's file, or, when there is no such den, the default one with a note
/// that says so.
pub fn resolve(folder: Option<&Path>, id: &str) -> (String, Loaded) {
    if let Some(prefab) = prefab(id) {
        return (
            prefab.id.to_owned(),
            Loaded {
                layout: prefab.layout,
                notes: Vec::new(),
            },
        );
    }
    let text = folder
        .filter(|_| valid_id(id))
        .and_then(|folder| std::fs::read_to_string(file(folder, id)).ok());
    match text {
        Some(text) => {
            let mut loaded = DenLayout::load(&text, &default_layout());
            if loaded
                .notes
                .first()
                .is_some_and(|note| note.starts_with("This is not a den"))
            {
                loaded.notes[0] = format!("{id}.json: {}", loaded.notes[0]);
                return (OFFICE.to_owned(), loaded);
            }
            (id.to_owned(), loaded)
        }
        None => (
            OFFICE.to_owned(),
            Loaded {
                layout: default_layout(),
                notes: vec![format!(
                    "There is no den called `{id}`: The office is used instead."
                )],
            },
        ),
    }
}

/// Writes a den to its file, whole or not at all: to a file beside it
/// first, which then takes its place.
pub fn write(folder: &Path, id: &str, layout: &DenLayout) -> io::Result<PathBuf> {
    if !valid_id(id) || is_prefab(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{id}` cannot be the name of a den's file"),
        ));
    }
    std::fs::create_dir_all(folder)?;
    let path = file(folder, id);
    let partial = folder.join(format!(".{id}.json.part"));
    std::fs::write(&partial, layout.to_json() + "\n")?;
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

/// Removes a den's file. A file that is not there is removed already.
pub fn remove(folder: &Path, id: &str) -> io::Result<()> {
    if !valid_id(id) {
        return Ok(());
    }
    match std::fs::remove_file(file(folder, id)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The name a den made from a built-in one is given the first time it is
/// changed: "The office" gives "My office".
pub fn own_name(prefab_name: &str) -> String {
    let rest = prefab_name.strip_prefix("The ").unwrap_or(prefab_name);
    format!("My {rest}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_den::layout::Placed;

    #[test]
    fn an_id_is_the_names_letters_and_digits_joined_by_dashes() {
        assert_eq!(id_for("My den"), "my-den");
        assert_eq!(id_for("  Café  №2 / attic!! "), "caf-2-attic");
        assert_eq!(id_for("../../etc/passwd"), "etc-passwd");
        assert_eq!(id_for("---"), "");
        assert_eq!(id_for(&"x".repeat(200)).len(), 48);
        assert!(valid_id("my-den") && valid_id("office"));
        for bad in ["", "My den", "../x", "a/b", "a..b", "-a", "a-", "a.json"] {
            assert!(!valid_id(bad), "{bad:?}");
        }
        assert_eq!(own_name("The office"), "My office");
        assert_eq!(own_name("Attic"), "My Attic");
    }

    #[test]
    fn a_new_den_never_takes_the_id_of_a_built_in_one_or_of_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        assert_eq!(free_id(Some(folder), "My den").as_deref(), Some("my-den"));
        assert_eq!(free_id(Some(folder), "Office").as_deref(), Some("office-2"));
        assert_eq!(free_id(None, "Office").as_deref(), Some("office-2"));
        assert_eq!(free_id(Some(folder), "!!!"), None);
        write(folder, "my-den", &default_layout()).unwrap();
        assert_eq!(free_id(Some(folder), "My den").as_deref(), Some("my-den-2"));
        write(folder, "my-den-2", &default_layout()).unwrap();
        assert_eq!(free_id(Some(folder), "my den").as_deref(), Some("my-den-3"));
    }

    #[test]
    fn a_den_is_written_to_its_file_and_read_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        // The folder is made when the first den is written.
        let folder = dir.path().join("dens");
        let mut den = default_layout();
        den.name = "My office".to_owned();
        den.items.push(Placed::new("pot", 5, 8));
        let path = write(&folder, "my-office", &den).unwrap();
        assert_eq!(path, folder.join("my-office.json"));
        let (id, loaded) = resolve(Some(&folder), "my-office");
        assert_eq!((id.as_str(), &loaded.layout), ("my-office", &den));
        assert_eq!(loaded.notes, Vec::<String>::new());
        // Nothing is left beside it.
        let files: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, ["my-office.json"]);
        // Written again, the file is replaced.
        den.items.pop();
        write(&folder, "my-office", &den).unwrap();
        assert_eq!(resolve(Some(&folder), "my-office").1.layout, den);
        // No file is written under a built-in den's id, or outside the folder.
        assert!(write(&folder, "office", &den).is_err());
        assert!(write(&folder, "../escape", &den).is_err());
        assert!(!dir.path().join("escape.json").exists());
    }

    #[test]
    fn a_den_that_is_not_there_or_is_no_den_gives_the_office_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let (id, loaded) = resolve(Some(folder), "attic");
        assert_eq!((id.as_str(), &loaded.layout), ("office", &default_layout()));
        assert_eq!(
            loaded.notes,
            ["There is no den called `attic`: The office is used instead."]
        );
        std::fs::write(folder.join("attic.json"), "{ not json").unwrap();
        let (id, loaded) = resolve(Some(folder), "attic");
        assert_eq!((id.as_str(), &loaded.layout), ("office", &default_layout()));
        assert!(loaded.notes[0].starts_with("attic.json: This is not a den"));
        // A den with a mistake in it is still that den, with a note.
        std::fs::write(
            folder.join("attic.json"),
            r#"{"name":"Attic","cols":10,"rows":9,"items":[{"id":"throne","x":2,"y":3}]}"#,
        )
        .unwrap();
        let (id, loaded) = resolve(Some(folder), "attic");
        assert_eq!(
            (id.as_str(), loaded.layout.name.as_str()),
            ("attic", "Attic")
        );
        assert_eq!(loaded.notes, ["There is no piece called `throne`."]);
        // A built-in den is itself whatever the folder holds, and an id that
        // is no id reads no file.
        std::fs::write(folder.join("nook.json"), "{}").unwrap();
        assert_eq!(resolve(Some(folder), "nook").1.layout.name, "The nook");
        assert_eq!(resolve(Some(folder), "../attic").0, "office");
        assert_eq!(resolve(None, "attic").0, "office");
        assert_eq!(resolve(None, "library").0, "library");
    }

    #[test]
    fn the_list_is_the_built_in_dens_then_the_users_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let built_in = list(None);
        assert!(built_in.len() >= 5 && built_in.iter().all(|den| !den.user));
        assert_eq!(built_in[0].id, "office");
        assert_eq!(list(Some(&folder.join("missing"))), built_in);

        let mut den = default_layout();
        den.name = "Zebra crossing".to_owned();
        write(folder, "zebra", &den).unwrap();
        den.name = "attic".to_owned();
        write(folder, "attic", &den).unwrap();
        std::fs::write(folder.join("broken.json"), "nope").unwrap();
        std::fs::write(folder.join("notes.txt"), "not a den").unwrap();
        std::fs::write(folder.join("Bad Name.json"), den.to_json()).unwrap();
        std::fs::write(folder.join("office.json"), den.to_json()).unwrap();
        let all = list(Some(folder));
        let own: Vec<(&str, &str, bool)> = all[built_in.len()..]
            .iter()
            .map(|den| (den.id.as_str(), den.name.as_str(), den.user))
            .collect();
        assert_eq!(
            own,
            [
                ("attic", "attic", true),
                ("broken", "broken", true),
                ("zebra", "Zebra crossing", true)
            ]
        );
        assert_eq!(all[built_in.len() + 1].detail, "This file is not a den.");
        assert!(all[built_in.len()]
            .detail
            .starts_with("Yours \u{b7} 20 by 15"));

        remove(folder, "zebra").unwrap();
        remove(folder, "zebra").unwrap();
        remove(folder, "../notes.txt").unwrap();
        assert_eq!(list(Some(folder)).len(), built_in.len() + 2);
    }

    #[test]
    fn the_guide_lists_every_den_every_piece_and_every_style() {
        let guide = include_str!("../../../../docs/DEN.md");
        for prefab in prefabs() {
            assert!(guide.contains(&format!("| `{}` | {} |", prefab.id, prefab.layout.name)));
        }
        for entry in leon_den::catalogue::CATALOGUE {
            let row = format!("| `{}` | ", entry.id);
            assert!(guide.contains(&row), "docs/DEN.md lacks {}", entry.id);
        }
        for floor in leon_den::assets::FLOORS {
            assert!(guide.contains(&format!("`{}`", floor.id)), "{}", floor.id);
        }
        for wall in leon_den::assets::WALLS {
            assert!(guide.contains(&format!("`{}`", wall.id)), "{}", wall.id);
        }
        // The example in it is a den that needs no repair.
        let start = guide.find("```json").unwrap() + "```json".len();
        let end = start + guide[start..].find("```").unwrap();
        let loaded = DenLayout::load(&guide[start..end], &default_layout());
        assert_eq!(loaded.notes, Vec::<String>::new());
        assert_eq!(loaded.layout.name, "Cave");
    }
}
