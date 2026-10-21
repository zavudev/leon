//! The host's way to King's service: what a paired device's lines to King
//! go through (`leon_host::king`).
//!
//! A connection is the one `leon king …` opens, with its checks of whose the
//! socket is, and it starts the service when nothing answers, the way the
//! command line does. The host passes the lines through and reads none.

use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use leon_host::{King, KingDoor, KingPipe};

use super::client::Client;
use super::Paths;

/// King's service for one data folder.
struct Door {
    paths: Paths,
}

impl KingDoor for Door {
    fn open(&self) -> Result<KingPipe, String> {
        let stream = Client::dial_or_start(&self.paths)?.into_stream();
        let write = stream.try_clone().map_err(|error| error.to_string())?;
        let closer = stream.try_clone().map_err(|error| error.to_string())?;
        Ok(KingPipe {
            write: Box::new(write),
            read: Box::new(BufReader::new(stream)),
            close: Box::new(move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            }),
        })
    }
}

/// The door to King's service of the data folder `data_dir`, for a host.
pub fn door(data_dir: &Path) -> King {
    King(Arc::new(Door {
        paths: Paths::here(data_dir),
    }))
}
