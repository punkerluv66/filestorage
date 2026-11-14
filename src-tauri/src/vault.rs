use crate::{
    transfer,
    wire::{REQUEST_TIMEOUT, read_message, write_message},
};
use anyhow::{Context, Result, ensure};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use iroh::endpoint::{RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};
use tokio::{fs::File, time::timeout};

pub type Reply<T> = std::result::Result<T, String>;

#[derive(Debug, Serialize, Deserialize)]
pub struct VaultFile {
    pub name: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize)]
pub enum Request {
    List,
    Upload(VaultFile),
    Download(String),
}

#[derive(Debug)]
pub struct Vault(Dir);

impl Vault {
    pub fn open(path: &Path) -> Result<Self> {
        let dir = Dir::open_ambient_dir(path, ambient_authority())
            .context("Could not open the selected vault folder")?;
        Ok(Self(dir))
    }

    pub async fn serve(self: Arc<Self>, mut send: SendStream, mut recv: RecvStream) -> Result<()> {
        match read_message(&mut recv, REQUEST_TIMEOUT).await? {
            Request::List => {
                let vault = self.clone();
                let files = tokio::task::spawn_blocking(move || vault.files()).await?;
                match files {
                    Ok(files) => {
                        write_message(&mut send, &Reply::Ok(())).await?;
                        for file in files {
                            write_message(&mut send, &Some(file)).await?;
                        }
                        write_message(&mut send, &Option::<VaultFile>::None).await?;
                    }
                    Err(error) => {
                        write_message(&mut send, &Reply::<()>::Err(error.to_string())).await?
                    }
                }
                send.finish()?;
            }
            Request::Upload(file) => self.upload(&mut send, &mut recv, file).await?,
            Request::Download(name) => self.download(&mut send, name).await?,
        }
        let _ = timeout(REQUEST_TIMEOUT, send.stopped()).await;
        Ok(())
    }

    fn files(&self) -> Result<Vec<VaultFile>> {
        let mut files = Vec::new();
        for entry in self
            .0
            .entries()
            .context("Could not read the vault folder")?
        {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if transfer::validate_name(&name).is_err() {
                continue;
            }
            let Ok(meta) = self.0.symlink_metadata(&name) else {
                continue;
            };
            if meta.is_file() && !is_reparse_point(&meta) {
                ensure!(files.len() < 100_000, "The folder listing is too large");
                files.push(VaultFile {
                    name,
                    size: meta.len(),
                });
            }
        }
        files.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    async fn upload(
        self: Arc<Self>,
        send: &mut SendStream,
        recv: &mut RecvStream,
        file: VaultFile,
    ) -> Result<()> {
        let vault = self.clone();
        let name = file.name;
        let upload = tokio::task::spawn_blocking(move || vault.stage_upload(&name)).await?;
        match upload {
            Ok((staged, output)) => {
                write_message(send, &Reply::Ok(())).await?;
                let result = async {
                    transfer::receive_file(recv, File::from_std(output), file.size).await?;
                    tokio::task::spawn_blocking(move || staged.publish()).await?
                }
                .await;
                write_message(send, &result.map_err(|error| error.to_string())).await?;
            }
            Err(error) => write_message(send, &Reply::<()>::Err(error.to_string())).await?,
        }
        send.finish()?;
        Ok(())
    }

    async fn download(self: Arc<Self>, send: &mut SendStream, name: String) -> Result<()> {
        let result = tokio::task::spawn_blocking(move || self.open_file(&name)).await?;
        match result {
            Ok((file, size)) => {
                write_message(send, &Reply::Ok(size)).await?;
                transfer::send_file(File::from_std(file), send, size).await?;
            }
            Err(error) => {
                write_message(send, &Reply::<u64>::Err(error.to_string())).await?;
                send.finish()?;
            }
        }
        Ok(())
    }

    fn open_file(&self, name: &str) -> Result<(std::fs::File, u64)> {
        transfer::validate_name(name)?;
        let meta = self
            .0
            .symlink_metadata(name)
            .context("File is unavailable")?;
        ensure!(
            meta.is_file() && !is_reparse_point(&meta),
            "File is unavailable"
        );
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = self
            .0
            .open_with(name, &options)
            .context("Could not open the requested file")?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && !is_reparse_point(&meta),
            "File is unavailable"
        );
        Ok((file.into_std(), meta.len()))
    }

    fn stage_upload(self: Arc<Self>, name: &str) -> Result<(StagedUpload, std::fs::File)> {
        transfer::validate_name(name)?;
        match self.0.symlink_metadata(name) {
            Ok(_) => anyhow::bail!("A file with that name already exists"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Could not check the destination"),
        }
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce)
            .map_err(|e| anyhow::anyhow!("Could not create an upload: {e}"))?;
        let temporary = format!(".pv-upload-{}", hex::encode(nonce));
        let file = self
            .0
            .open_with(&temporary, OpenOptions::new().write(true).create_new(true))
            .context("Could not start writing to the vault folder")?;
        Ok((
            StagedUpload {
                vault: self,
                temporary,
                name: name.to_owned(),
            },
            file.into_std(),
        ))
    }
}

struct StagedUpload {
    vault: Arc<Vault>,
    temporary: String,
    name: String,
}

impl StagedUpload {
    fn publish(self) -> Result<()> {
        self.vault.0.hard_link(&self.temporary, &self.vault.0, &self.name)
            .context("Could not publish the file. The vault needs a filesystem that supports hard links and an unused filename")?;
        Ok(())
    }
}

impl Drop for StagedUpload {
    fn drop(&mut self) {
        let _ = self.vault.0.remove_file(&self.temporary);
    }
}

#[cfg(windows)]
fn is_reparse_point(meta: &cap_std::fs::Metadata) -> bool {
    use cap_fs_ext::OsMetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_meta: &cap_std::fs::Metadata) -> bool {
    false
}
