//! Project logos: what detection chose and what the user chose.
//!
//! A project has up to two rows. The *detected* one is written by the engine
//! when a project is added or discovered and when the user asks to refresh
//! it; the *custom* one is the user's own file. Reading a project's icon
//! returns the custom one when there is one, so detecting again never undoes a
//! choice and resetting just removes it. Image bytes live in their own column
//! and are read by content hash, so the lists the tree is built from stay
//! small.

use std::collections::HashMap;

use rusqlite::{params, Transaction};

use super::{to_millis, Store};
use crate::change::StoreChange;
use crate::error::{Result, StoreError};
use crate::icon::{IconFormat, IconImage, IconKind, NewIcon, ProjectIcon};
use crate::ids::ProjectId;

use chrono::Utc;

const DETECTED: &str = "detected";
const CUSTOM: &str = "custom";

impl Store {
    /// Records what detection found for a project (`remote` is the origin's
    /// `host/owner/repo`, when it has one). Replaces an earlier detection;
    /// a custom choice is untouched.
    pub fn set_detected_icon(
        &self,
        project: &ProjectId,
        icon: &NewIcon,
        remote: Option<&str>,
    ) -> Result<()> {
        self.write(StoreChange::Projects, |tx| {
            put(tx, project, DETECTED, icon, remote)
        })
    }

    /// Records the image a user chose for a project, named `source` (its file
    /// name).
    pub fn set_custom_icon(
        &self,
        project: &ProjectId,
        image: IconImage,
        source: &str,
    ) -> Result<()> {
        let icon = NewIcon {
            kind: IconKind::Custom,
            source: source.to_owned(),
            image: Some(image),
        };
        self.write(StoreChange::Projects, |tx| {
            put(tx, project, CUSTOM, &icon, None)
        })
    }

    /// Forgets the user's choice: the detected logo shows again.
    pub fn clear_custom_icon(&self, project: &ProjectId) -> Result<()> {
        self.write(StoreChange::Projects, |tx| {
            tx.prepare_cached("DELETE FROM project_icon WHERE project_id = ?1 AND origin = ?2")?
                .execute(params![project.as_str(), CUSTOM])?;
            Ok(())
        })
    }

    /// The logo in effect of every project that has one: the user's choice,
    /// else the detected one. One read, no image bytes.
    pub fn project_icons(&self) -> Result<HashMap<ProjectId, ProjectIcon>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT project_id, origin, kind, source, format, hash, remote FROM project_icon",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut icons: HashMap<ProjectId, ProjectIcon> = HashMap::new();
            let mut remotes: HashMap<ProjectId, String> = HashMap::new();
            for (project, origin, kind, source, format, hash, remote) in rows {
                let id = ProjectId::from_string(project);
                let kind = IconKind::parse(&kind)
                    .ok_or_else(|| StoreError::Corrupt(format!("icon kind {kind:?}")))?;
                if let Some(remote) = remote {
                    remotes.insert(id.clone(), remote);
                }
                let icon = ProjectIcon {
                    project_id: id.clone(),
                    kind,
                    source,
                    format: format.as_deref().and_then(IconFormat::parse),
                    hash,
                    remote: None,
                };
                // The custom row wins over the detected one.
                if origin == CUSTOM || !icons.contains_key(&id) {
                    icons.insert(id, icon);
                }
            }
            for (id, icon) in &mut icons {
                icon.remote = remotes.remove(id);
            }
            Ok(icons)
        })
    }

    /// The image with this content hash, with its format.
    pub fn icon_image(&self, hash: &str) -> Result<Option<IconImage>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT format, image FROM project_icon WHERE hash = ?1 AND image IS NOT NULL LIMIT 1",
            )?;
            let found = statement
                .query_map([hash], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                })?
                .next()
                .transpose()?;
            Ok(found.and_then(|(format, bytes)| {
                IconFormat::parse(&format).map(|format| IconImage { format, bytes })
            }))
        })
    }

    /// The projects nothing has been detected for yet: those added before
    /// logos existed and those whose detection failed.
    pub fn projects_without_detected_icon(&self) -> Result<Vec<ProjectId>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT p.id FROM project p
                 WHERE NOT EXISTS (SELECT 1 FROM project_icon i
                                   WHERE i.project_id = p.id AND i.origin = 'detected')
                 ORDER BY p.name COLLATE NOCASE, p.id",
            )?;
            let ids = statement
                .query_map([], |row| {
                    Ok(ProjectId::from_string(row.get::<_, String>(0)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(ids)
        })
    }
}

fn put(
    tx: &Transaction<'_>,
    project: &ProjectId,
    origin: &str,
    icon: &NewIcon,
    remote: Option<&str>,
) -> Result<()> {
    let exists = tx
        .prepare_cached("SELECT 1 FROM project WHERE id = ?1")?
        .exists([project.as_str()])?;
    if !exists {
        return Err(StoreError::NotFound("project"));
    }
    let (format, hash, bytes) = match &icon.image {
        Some(image) => (
            Some(image.format.as_str()),
            Some(crate::icon::content_hash(&image.bytes)),
            Some(image.bytes.as_slice()),
        ),
        None => (None, None, None),
    };
    tx.prepare_cached(
        "INSERT INTO project_icon (project_id, origin, kind, source, format, hash, image, remote, set_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (project_id, origin) DO UPDATE SET
             kind = excluded.kind, source = excluded.source, format = excluded.format,
             hash = excluded.hash, image = excluded.image, remote = excluded.remote,
             set_at = excluded.set_at",
    )?
    .execute(params![
        project.as_str(),
        origin,
        icon.kind.as_str(),
        icon.source,
        format,
        hash,
        bytes,
        remote,
        to_millis(Utc::now()),
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MachineId;

    fn png() -> IconImage {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend(16u32.to_be_bytes());
        bytes.extend(16u32.to_be_bytes());
        bytes.extend([8, 6, 0, 0, 0]);
        IconImage::from_bytes(bytes).unwrap()
    }

    fn detected(source: &str) -> NewIcon {
        NewIcon {
            kind: IconKind::Detected,
            source: source.into(),
            image: Some(png()),
        }
    }

    fn project(store: &Store) -> ProjectId {
        store
            .add_project(&MachineId::local(), "api", "/srv/api")
            .unwrap()
            .id
    }

    #[test]
    fn a_detected_icon_is_stored_with_its_source_and_remote_and_read_back_without_bytes() {
        let store = Store::open_in_memory().unwrap();
        let id = project(&store);
        store
            .set_detected_icon(
                &id,
                &detected("nextjs-app:app/icon.png"),
                Some("github.com/zavu/api"),
            )
            .unwrap();
        let icons = store.project_icons().unwrap();
        let icon = &icons[&id];
        assert_eq!(icon.kind, IconKind::Detected);
        assert_eq!(icon.source, "nextjs-app:app/icon.png");
        assert_eq!(icon.format, Some(IconFormat::Png));
        assert_eq!(icon.remote.as_deref(), Some("github.com/zavu/api"));
        let image = store
            .icon_image(icon.hash.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(image, png());
    }

    #[test]
    fn a_users_choice_wins_over_detection_and_detecting_again_does_not_undo_it() {
        let store = Store::open_in_memory().unwrap();
        let id = project(&store);
        store
            .set_detected_icon(&id, &detected("a:b.png"), Some("github.com/z/a"))
            .unwrap();
        store.set_custom_icon(&id, png(), "mine.png").unwrap();
        assert_eq!(store.project_icons().unwrap()[&id].kind, IconKind::Custom);

        store
            .set_detected_icon(&id, &detected("c:d.png"), Some("github.com/z/a"))
            .unwrap();
        let icon = store.project_icons().unwrap().remove(&id).unwrap();
        assert_eq!(icon.kind, IconKind::Custom);
        assert_eq!(icon.source, "mine.png");
        assert_eq!(
            icon.remote.as_deref(),
            Some("github.com/z/a"),
            "the remote still comes from detection"
        );
    }

    #[test]
    fn resetting_brings_the_detected_icon_back() {
        let store = Store::open_in_memory().unwrap();
        let id = project(&store);
        store
            .set_detected_icon(&id, &detected("a:b.png"), None)
            .unwrap();
        store.set_custom_icon(&id, png(), "mine.png").unwrap();
        store.clear_custom_icon(&id).unwrap();
        let icon = store.project_icons().unwrap().remove(&id).unwrap();
        assert_eq!(
            (icon.kind, icon.source.as_str()),
            (IconKind::Detected, "a:b.png")
        );
    }

    #[test]
    fn a_folder_result_is_stored_without_an_image() {
        let store = Store::open_in_memory().unwrap();
        let id = project(&store);
        let none = NewIcon {
            kind: IconKind::Folder,
            source: String::new(),
            image: None,
        };
        store.set_detected_icon(&id, &none, None).unwrap();
        let icon = store.project_icons().unwrap().remove(&id).unwrap();
        assert_eq!((icon.kind, icon.hash), (IconKind::Folder, None));
    }

    #[test]
    fn only_projects_never_detected_are_listed_for_detection() {
        let store = Store::open_in_memory().unwrap();
        let first = project(&store);
        let second = store
            .add_project(&MachineId::local(), "web", "/srv/web")
            .unwrap()
            .id;
        store
            .set_detected_icon(&first, &detected("a:b.png"), None)
            .unwrap();
        assert_eq!(
            store.projects_without_detected_icon().unwrap(),
            vec![second]
        );
    }

    #[test]
    fn removing_a_project_removes_its_icons() {
        let store = Store::open_in_memory().unwrap();
        let id = project(&store);
        store
            .set_detected_icon(&id, &detected("a:b.png"), None)
            .unwrap();
        store.remove_project(&id).unwrap();
        assert!(store.project_icons().unwrap().is_empty());
    }

    #[test]
    fn an_icon_for_an_unknown_project_is_refused_and_a_write_announces_projects() {
        let store = Store::open_in_memory().unwrap();
        assert!(matches!(
            store.set_detected_icon(&ProjectId::generate(), &detected("a:b.png"), None),
            Err(StoreError::NotFound("project"))
        ));
        let id = project(&store);
        let mut listener = store.subscribe();
        store
            .set_detected_icon(&id, &detected("a:b.png"), None)
            .unwrap();
        assert_eq!(listener.try_next(), Some(StoreChange::Projects));
    }
}
