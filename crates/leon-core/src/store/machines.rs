//! Machine persistence.
//!
//! Machines are the roots of everything else: projects, sessions and import
//! cursors all hang off a machine and disappear with it. The built-in local
//! machine is created by the first migration and can be renamed but never
//! removed or turned into something else.

use rusqlite::{params, OptionalExtension, Row};

use super::{bad_tag, Store};
use crate::change::StoreChange;
use crate::error::{Result, StoreError};
use crate::ids::MachineId;
use crate::model::{Machine, MachineKind};

const COLUMNS: &str = "id, name, kind, host, user, port, identity_file";

impl Store {
    /// Every machine, the local one first and the rest by name.
    pub fn machines(&self) -> Result<Vec<Machine>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM machine
                 ORDER BY kind = 'local' DESC, name COLLATE NOCASE, id"
            ))?;
            let machines = statement
                .query_map([], machine_from_row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(machines)
        })
    }

    /// The machine with the given id.
    pub fn machine(&self, id: &MachineId) -> Result<Machine> {
        self.read(|connection| {
            connection
                .prepare_cached(&format!("SELECT {COLUMNS} FROM machine WHERE id = ?1"))?
                .query_row([id.as_str()], machine_from_row)
                .optional()?
                .ok_or(StoreError::NotFound("machine"))
        })
    }

    /// Registers a machine reached over SSH. A second local machine is
    /// refused: the built-in one is the only one there can be.
    pub fn add_machine(&self, name: &str, kind: MachineKind) -> Result<Machine> {
        if kind == MachineKind::Local {
            return Err(StoreError::Invalid(
                "the local machine already exists".into(),
            ));
        }
        let machine = Machine {
            id: MachineId::generate(),
            name: name.to_owned(),
            kind,
        };
        self.write(StoreChange::Machines, |tx| {
            let (host, user, port, identity_file) = ssh_columns(&machine.kind);
            tx.prepare_cached(
                "INSERT INTO machine (id, name, kind, host, user, port, identity_file)
                 VALUES (?1, ?2, 'ssh', ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                machine.id.as_str(),
                machine.name,
                host,
                user,
                port,
                identity_file
            ])?;
            Ok(())
        })?;
        Ok(machine)
    }

    /// Replaces the name and connection details of an existing machine. The
    /// kind itself cannot change: a local machine stays local and an SSH
    /// machine stays an SSH machine.
    pub fn update_machine(&self, machine: &Machine) -> Result<()> {
        if machine.id.is_local() != (machine.kind == MachineKind::Local) {
            return Err(StoreError::Invalid(
                "a machine cannot change between local and SSH".into(),
            ));
        }
        self.write(StoreChange::Machines, |tx| {
            let (host, user, port, identity_file) = ssh_columns(&machine.kind);
            let updated = tx
                .prepare_cached(
                    "UPDATE machine
                     SET name = ?2, host = ?3, user = ?4, port = ?5, identity_file = ?6
                     WHERE id = ?1",
                )?
                .execute(params![
                    machine.id.as_str(),
                    machine.name,
                    host,
                    user,
                    port,
                    identity_file
                ])?;
            if updated == 0 {
                return Err(StoreError::NotFound("machine"));
            }
            Ok(())
        })
    }

    /// Removes a machine together with its projects, worktrees, sessions and
    /// import cursors. The local machine cannot be removed.
    pub fn remove_machine(&self, id: &MachineId) -> Result<()> {
        if id.is_local() {
            return Err(StoreError::Invalid(
                "the local machine cannot be removed".into(),
            ));
        }
        self.write(StoreChange::Everything, |tx| {
            let removed = tx
                .prepare_cached("DELETE FROM machine WHERE id = ?1")?
                .execute([id.as_str()])?;
            if removed == 0 {
                return Err(StoreError::NotFound("machine"));
            }
            Ok(())
        })
    }
}

type SshColumns<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    Option<u16>,
    Option<&'a str>,
);

fn ssh_columns(kind: &MachineKind) -> SshColumns<'_> {
    match kind {
        MachineKind::Local => (None, None, None, None),
        MachineKind::Ssh {
            host,
            user,
            port,
            identity_file,
        } => (
            Some(host.as_str()),
            user.as_deref(),
            *port,
            identity_file.as_deref(),
        ),
    }
}

fn machine_from_row(row: &Row<'_>) -> rusqlite::Result<Machine> {
    let tag: String = row.get(2)?;
    let kind = match tag.as_str() {
        "local" => MachineKind::Local,
        "ssh" => MachineKind::Ssh {
            host: row.get(3)?,
            user: row.get(4)?,
            port: row.get(5)?,
            identity_file: row.get(6)?,
        },
        other => return Err(bad_tag(2, other)),
    };
    Ok(Machine {
        id: MachineId::from_string(row.get::<_, String>(0)?),
        name: row.get(1)?,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh(host: &str) -> MachineKind {
        MachineKind::Ssh {
            host: host.into(),
            user: Some("dev".into()),
            port: Some(2222),
            identity_file: Some("/keys/id_ed25519".into()),
        }
    }

    #[test]
    fn an_added_machine_is_read_back_with_all_its_connection_details() {
        let store = Store::open_in_memory().unwrap();
        let added = store
            .add_machine("build box", ssh("build.example"))
            .unwrap();
        assert_eq!(store.machine(&added.id).unwrap(), added);
    }

    #[test]
    fn machines_are_listed_with_the_local_one_first_then_by_name() {
        let store = Store::open_in_memory().unwrap();
        store.add_machine("zeta", ssh("z.example")).unwrap();
        store.add_machine("Alpha", ssh("a.example")).unwrap();
        let names: Vec<_> = store
            .machines()
            .unwrap()
            .into_iter()
            .map(|machine| machine.name)
            .collect();
        assert_eq!(names, ["This machine", "Alpha", "zeta"]);
    }

    #[test]
    fn adding_a_second_local_machine_is_refused() {
        let store = Store::open_in_memory().unwrap();
        assert!(matches!(
            store.add_machine("another", MachineKind::Local),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn updating_a_machine_replaces_its_details() {
        let store = Store::open_in_memory().unwrap();
        let mut machine = store
            .add_machine("build box", ssh("build.example"))
            .unwrap();
        machine.name = "renamed".into();
        machine.kind = MachineKind::Ssh {
            host: "other.example".into(),
            user: None,
            port: None,
            identity_file: None,
        };
        store.update_machine(&machine).unwrap();
        assert_eq!(store.machine(&machine.id).unwrap(), machine);
    }

    #[test]
    fn the_local_machine_can_be_renamed_but_not_turned_into_an_ssh_machine() {
        let store = Store::open_in_memory().unwrap();
        let mut local = store.machine(&MachineId::local()).unwrap();
        local.name = "Laptop".into();
        store.update_machine(&local).unwrap();
        assert_eq!(store.machine(&MachineId::local()).unwrap().name, "Laptop");

        local.kind = ssh("elsewhere.example");
        assert!(matches!(
            store.update_machine(&local),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn updating_an_unknown_machine_reports_not_found() {
        let store = Store::open_in_memory().unwrap();
        let ghost = Machine {
            id: MachineId::generate(),
            name: "ghost".into(),
            kind: ssh("ghost.example"),
        };
        assert!(matches!(
            store.update_machine(&ghost),
            Err(StoreError::NotFound("machine"))
        ));
    }

    #[test]
    fn the_local_machine_cannot_be_removed() {
        let store = Store::open_in_memory().unwrap();
        assert!(matches!(
            store.remove_machine(&MachineId::local()),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(store.machines().unwrap().len(), 1);
    }

    #[test]
    fn removing_a_machine_removes_its_projects() {
        let store = Store::open_in_memory().unwrap();
        let machine = store
            .add_machine("build box", ssh("build.example"))
            .unwrap();
        store.add_project(&machine.id, "api", "/srv/api").unwrap();
        store.remove_machine(&machine.id).unwrap();
        assert!(store.projects(None).unwrap().is_empty());
        assert!(matches!(
            store.machine(&machine.id),
            Err(StoreError::NotFound("machine"))
        ));
    }
}
