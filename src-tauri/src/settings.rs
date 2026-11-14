use anyhow::{Context, Result};
use iroh::{EndpointAddr, EndpointId};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub folder: Option<PathBuf>,
    pub clients: BTreeSet<EndpointId>,
    pub hosts: BTreeMap<EndpointId, EndpointAddr>,
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).context("The vault settings could not be read")
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).context("Could not open the vault settings"),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let folder = path
            .parent()
            .context("The settings directory is unavailable")?;
        fs::create_dir_all(folder)?;
        let mut file = tempfile::NamedTempFile::new_in(folder)?;
        serde_json::to_writer(&mut file, self)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(path)
            .context("Could not replace the vault settings")?;
        Ok(())
    }
}
