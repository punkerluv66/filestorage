use crate::{
    settings::Settings,
    transfer,
    vault::{Reply, Request, Vault, VaultFile},
    wire::{DECISION_TIMEOUT, REQUEST_TIMEOUT, read_message, write_message},
};
use anyhow::{Context, Result, bail, ensure};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, Watcher,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_tickets::endpoint::EndpointTicket;
use serde::Serialize;
use std::{
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    fs::File,
    sync::{Semaphore, oneshot},
    time::timeout,
};

const PAIR_ALPN: &[u8] = b"/p2p/pair/1";
const VAULT_ALPN: &[u8] = b"/p2p/vault/1";
const INVITE_PREFIX: &str = "vault1";
const INVITE_LIFETIME: Duration = Duration::from_secs(300);
type Token = [u8; 32];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Pairing {
    peer_id: EndpointId,
    safety_code: String,
    incoming: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    endpoint_id: EndpointId,
    folder: Option<PathBuf>,
    relay_connected: bool,
    inviting: bool,
    pairing: Option<Pairing>,
    clients: Vec<EndpointId>,
    hosts: Vec<EndpointId>,
    startup_error: Option<String>,
}

struct State {
    settings: Settings,
    vault: Option<Arc<Vault>>,
    startup_error: Option<String>,
    invitation: Option<(Token, Instant)>,
    pairing: Option<(Pairing, Option<oneshot::Sender<bool>>)>,
    connections: Vec<Connection>,
}

struct Access {
    endpoint_id: EndpointId,
    file: PathBuf,
    state: Mutex<State>,
    handshakes: Semaphore,
    transfers: Semaphore,
}

impl fmt::Debug for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Vault access")
    }
}

impl Access {
    fn change_settings(&self, state: &mut State, edit: impl FnOnce(&mut Settings)) -> Result<()> {
        let mut settings = state.settings.clone();
        edit(&mut settings);
        settings.save(&self.file)?;
        state.settings = settings;
        Ok(())
    }

    async fn accept_invite(&self, conn: &Connection) -> Result<()> {
        let _slot = self
            .handshakes
            .try_acquire()
            .context("Too many pairing requests")?;
        let (mut send, mut recv) = timeout(REQUEST_TIMEOUT, conn.accept_bi()).await??;
        let token: Token = read_message(&mut recv, REQUEST_TIMEOUT).await?;
        let peer_id = conn.remote_id();
        let response = {
            let mut state = self.state.lock().unwrap();
            ensure!(
                state.vault.is_some() && state.pairing.is_none(),
                "Pairing is unavailable"
            );
            ensure!(
                state
                    .invitation
                    .as_ref()
                    .is_some_and(|(expected, expiry)| *expiry > Instant::now()
                        && bool::from(subtle::ConstantTimeEq::ct_eq(
                            expected.as_slice(),
                            token.as_slice()
                        ))),
                "Invalid or expired invitation"
            );
            state.invitation = None;
            let (decision, response) = oneshot::channel();
            state.pairing = Some((
                make_pairing(&token, peer_id, self.endpoint_id, true),
                Some(decision),
            ));
            response
        };

        let result: Result<()> = async {
            let approved = tokio::select! {
                approval = timeout(DECISION_TIMEOUT, response) => approval??,
                _ = conn.closed() => bail!("The other device disconnected"),
            };
            if !approved {
                bail!("The host declined pairing");
            }
            let mut state = self.state.lock().unwrap();
            self.change_settings(&mut state, |settings| {
                settings.clients.insert(peer_id);
            })?;
            Ok(())
        }
        .await;
        self.state.lock().unwrap().pairing = None;
        write_message(&mut send, &result.map_err(|error| error.to_string())).await?;
        send.finish()?;
        let _ = timeout(REQUEST_TIMEOUT, send.stopped()).await;
        Ok(())
    }

    async fn serve_request(&self, conn: &Connection) -> Result<()> {
        let _slot = self.transfers.try_acquire().context("The vault is busy")?;
        let vault = {
            let mut state = self.state.lock().unwrap();
            ensure!(
                state.settings.clients.contains(&conn.remote_id()),
                "This device is not authorized"
            );
            let vault = state.vault.clone().context("The vault is unavailable")?;
            state.connections.push(conn.clone());
            vault
        };
        let result = async {
            let (send, recv) = timeout(REQUEST_TIMEOUT, conn.accept_bi()).await??;
            tokio::select! {
                result = vault.serve(send, recv) => result,
                _ = conn.closed() => bail!("The connection closed"),
            }
        }
        .await;
        self.state
            .lock()
            .unwrap()
            .connections
            .retain(|active| active.stable_id() != conn.stable_id());
        result
    }
}

impl ProtocolHandler for Access {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let result = if conn.alpn() == PAIR_ALPN {
            self.accept_invite(&conn).await
        } else {
            self.serve_request(&conn).await
        };
        if result.is_err() {
            conn.close(1u32.into(), b"vault request rejected");
        }
        Ok(())
    }
}

pub struct Network {
    router: Router,
    access: Arc<Access>,
}

impl Network {
    pub async fn bind(key: SecretKey, file: PathBuf) -> Result<Self> {
        let builder = match std::env::var("FILESTORAGE_NETWORK").as_deref() {
            Ok("direct") => Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled),
            Ok("relay") => Endpoint::builder(presets::N0).clear_ip_transports(),
            _ => Endpoint::builder(presets::N0),
        };
        Self::bind_with(key, file, builder).await
    }

    async fn bind_with(
        key: SecretKey,
        file: PathBuf,
        builder: iroh::endpoint::Builder,
    ) -> Result<Self> {
        let (settings, vault, startup_error) = tokio::task::spawn_blocking({
            let file = file.clone();
            move || -> Result<_> {
                let settings = Settings::load(&file)?;
                let (vault, error) = match settings.folder.as_deref().map(Vault::open).transpose() {
                    Ok(vault) => (vault.map(Arc::new), None),
                    Err(error) => (None, Some(format!("{error:#}"))),
                };
                Ok((settings, vault, error))
            }
        })
        .await??;
        let endpoint = builder
            .secret_key(key)
            .bind()
            .await
            .context("Could not start the vault network")?;
        let access = Arc::new(Access {
            endpoint_id: endpoint.id(),
            file,
            handshakes: Semaphore::new(4),
            transfers: Semaphore::new(32),
            state: Mutex::new(State {
                settings,
                vault,
                startup_error,
                invitation: None,
                pairing: None,
                connections: Vec::new(),
            }),
        });
        let router = Router::builder(endpoint)
            .accept(PAIR_ALPN, access.clone())
            .accept(VAULT_ALPN, access.clone())
            .spawn();
        Ok(Self { router, access })
    }

    pub fn status(&self) -> Status {
        let endpoint = self.router.endpoint();
        let state = self.access.state.lock().unwrap();
        Status {
            endpoint_id: endpoint.id(),
            folder: state.settings.folder.clone(),
            relay_connected: endpoint
                .home_relay_status()
                .get()
                .iter()
                .any(|relay| relay.is_connected()),
            inviting: state
                .invitation
                .as_ref()
                .is_some_and(|(_, expiry)| *expiry > Instant::now()),
            pairing: state.pairing.as_ref().map(|(pairing, _)| pairing.clone()),
            clients: state.settings.clients.iter().copied().collect(),
            hosts: state.settings.hosts.keys().copied().collect(),
            startup_error: state.startup_error.clone(),
        }
    }

    pub fn set_folder(&self, folder: PathBuf) -> Result<()> {
        let vault = Arc::new(Vault::open(&folder)?);
        let mut state = self.access.state.lock().unwrap();
        ensure!(state.pairing.is_none(), "Finish the current pairing first");
        self.access.change_settings(&mut state, |settings| {
            settings.folder = Some(folder);
        })?;
        state.invitation = None;
        state.startup_error = None;
        state.vault = Some(vault);
        for conn in state.connections.drain(..) {
            conn.close(0u32.into(), b"vault folder changed");
        }
        Ok(())
    }

    pub fn create_invite(&self) -> Result<String> {
        let mut state = self.access.state.lock().unwrap();
        ensure!(state.vault.is_some(), "Choose a storage folder first");
        ensure!(state.pairing.is_none(), "Finish the current pairing first");
        let address = self.router.endpoint().addr();
        ensure!(
            !address.is_empty(),
            "No network address is ready yet. Try again shortly."
        );
        let mut token = [0; 32];
        getrandom::fill(&mut token)
            .map_err(|error| anyhow::anyhow!("Could not create an invitation: {error}"))?;
        state.invitation = Some((token, Instant::now() + INVITE_LIFETIME));
        Ok(format!(
            "{INVITE_PREFIX}.{}.{}",
            hex::encode(token),
            EndpointTicket::new(address)
        ))
    }

    pub fn cancel_invite(&self) {
        self.access.state.lock().unwrap().invitation = None;
    }

    pub async fn pair(&self, invite: &str) -> Result<EndpointId> {
        let invite = invite.trim();
        ensure!(invite.len() <= 4096, "The invitation is too long");
        let mut parts = invite.splitn(3, '.');
        ensure!(
            parts.next() == Some(INVITE_PREFIX),
            "Enter a Personal Vault invitation"
        );
        let mut token = [0; 32];
        hex::decode_to_slice(parts.next().context("Invalid invitation")?, &mut token)
            .context("Invalid invitation token")?;
        let ticket: EndpointTicket = parts
            .next()
            .context("Missing device address")?
            .parse()
            .context("Invalid device address")?;
        let address = ticket.endpoint_addr().clone();
        let endpoint = self.router.endpoint();
        ensure!(
            address.id != endpoint.id(),
            "Use this invitation on another device"
        );
        let host_id = address.id;
        {
            let mut state = self.access.state.lock().unwrap();
            ensure!(state.pairing.is_none(), "Finish the current pairing first");
            state.invitation = None;
            state.pairing = Some((make_pairing(&token, host_id, endpoint.id(), false), None));
        }
        let result = async {
            let connection = timeout(
                REQUEST_TIMEOUT,
                endpoint.connect(address.clone(), PAIR_ALPN),
            )
            .await?
            .context("Could not reach the host")?;
            let (mut send, mut recv) = timeout(REQUEST_TIMEOUT, connection.open_bi()).await??;
            write_message(&mut send, &token).await?;
            send.finish()?;
            check_reply::<()>(read_message(&mut recv, DECISION_TIMEOUT).await?)?;
            let mut state = self.access.state.lock().unwrap();
            self.access.change_settings(&mut state, |settings| {
                settings.hosts.insert(host_id, address);
            })
        }
        .await;
        self.access.state.lock().unwrap().pairing = None;
        result?;
        Ok(host_id)
    }

    pub fn confirm(&self, peer_id: EndpointId, approved: bool) -> Result<()> {
        let mut state = self.access.state.lock().unwrap();
        let (pairing, decision) = state
            .pairing
            .as_mut()
            .context("There is no pending device")?;
        ensure!(
            pairing.peer_id == peer_id,
            "This pairing request has expired"
        );
        decision
            .take()
            .context("This request has already been answered")?
            .send(approved)
            .map_err(|_| anyhow::anyhow!("The other device disconnected"))
    }

    pub fn revoke(&self, peer_id: EndpointId) -> Result<()> {
        let mut state = self.access.state.lock().unwrap();
        self.access.change_settings(&mut state, |settings| {
            settings.clients.remove(&peer_id);
        })?;
        state.connections.retain(|conn| {
            if conn.remote_id() == peer_id {
                conn.close(1u32.into(), b"access revoked");
                false
            } else {
                true
            }
        });
        Ok(())
    }

    async fn request(
        &self,
        host: EndpointId,
        request: Request,
    ) -> Result<(Connection, SendStream, RecvStream)> {
        let address = self
            .access
            .state
            .lock()
            .unwrap()
            .settings
            .hosts
            .get(&host)
            .cloned()
            .context("Pair with the storage device first")?;
        let endpoint = self.router.endpoint();
        // Lookup resolves the current relay after IP or network changes. Keep invitation addresses as a LAN fallback.
        let by_id = endpoint.connect(EndpointAddr::new(host), VAULT_ALPN);
        let by_address = endpoint.connect(address, VAULT_ALPN);
        let connection = timeout(REQUEST_TIMEOUT, async {
            tokio::pin!(by_id, by_address);
            tokio::select! {
                first = &mut by_id => match first { Ok(conn) => Ok(conn), Err(_) => by_address.await },
                first = &mut by_address => match first { Ok(conn) => Ok(conn), Err(_) => by_id.await },
            }
        }).await.context("The storage device is unreachable")??;
        let (mut send, recv) = timeout(REQUEST_TIMEOUT, connection.open_bi()).await??;
        write_message(&mut send, &request).await?;
        Ok((connection, send, recv))
    }

    pub async fn files(&self, host: EndpointId) -> Result<Vec<VaultFile>> {
        let (_connection, mut send, mut recv) = self.request(host, Request::List).await?;
        send.finish()?;
        check_reply::<()>(read_message(&mut recv, REQUEST_TIMEOUT).await?)?;
        let mut files = Vec::new();
        while let Some(file) = read_message(&mut recv, REQUEST_TIMEOUT).await? {
            ensure!(files.len() < 100_000, "The folder listing is too large");
            files.push(file);
        }
        Ok(files)
    }

    pub async fn upload(&self, host: EndpointId, path: PathBuf) -> Result<()> {
        let file = File::open(&path)
            .await
            .context("Could not open the selected file")?;
        let meta = file.metadata().await?;
        ensure!(meta.is_file(), "Select a regular file");
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("The source filename is unsupported")?;
        transfer::validate_name(name)?;
        let (_conn, mut send, mut recv) = self
            .request(
                host,
                Request::Upload(VaultFile {
                    name: name.to_owned(),
                    size: meta.len(),
                }),
            )
            .await?;
        check_reply::<()>(read_message(&mut recv, REQUEST_TIMEOUT).await?)?;
        transfer::send_file(file, &mut send, meta.len()).await?;
        check_reply(read_message(&mut recv, REQUEST_TIMEOUT).await?)
    }

    pub async fn download(&self, host: EndpointId, name: String, path: PathBuf) -> Result<()> {
        transfer::validate_name(&name)?;
        let folder = path.parent().context("Choose a destination folder")?;
        let temp = tempfile::NamedTempFile::new_in(folder)?;
        let (_conn, mut send, mut recv) = self.request(host, Request::Download(name)).await?;
        send.finish()?;
        let size: u64 = check_reply(read_message(&mut recv, REQUEST_TIMEOUT).await?)?;
        transfer::receive_file(&mut recv, File::from_std(temp.reopen()?), size).await?;
        temp.persist_noclobber(path)
            .context("Could not save the download. Choose an unused filename")?;
        Ok(())
    }
}

fn check_reply<T>(reply: Reply<T>) -> Result<T> {
    reply.map_err(anyhow::Error::msg)
}

fn make_pairing(
    token: &Token,
    peer_id: EndpointId,
    local_id: EndpointId,
    incoming: bool,
) -> Pairing {
    let mut identities = [peer_id, local_id];
    identities.sort_unstable();
    let mut hash = blake3::Hasher::new_keyed(token);
    hash.update(b"personal-vault/pair/safety-code/1");
    for id in identities {
        hash.update(id.as_bytes());
    }
    let digest = hash.finalize();
    let prefix = u32::from_be_bytes(digest.as_bytes()[..4].try_into().unwrap());
    Pairing {
        peer_id,
        safety_code: format!("{:06}", prefix % 1_000_000),
        incoming,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::time::sleep;

    #[tokio::test]
    async fn authorized_access_survives_restart_and_supports_upload_list_download_and_revocation()
    -> Result<()> {
        let sandbox = tempfile::tempdir()?;
        let root = sandbox.path().join("vault");
        std::fs::create_dir(&root)?;
        let host_key = SecretKey::generate();
        let client_key = SecretKey::generate();
        let host_id = host_key.public();
        let host_file = sandbox.path().join("host.json");
        let client_file = sandbox.path().join("client.json");
        let host = Network::bind_with(host_key.clone(), host_file.clone(), lan()).await?;
        let client = Network::bind_with(client_key.clone(), client_file.clone(), lan()).await?;
        host.set_folder(root.clone())?;

        let invite = host.create_invite()?;
        let ip = SocketAddr::from((
            Ipv4Addr::LOCALHOST,
            host.router
                .endpoint()
                .bound_sockets()
                .into_iter()
                .find(|addr| addr.is_ipv4())
                .unwrap()
                .port(),
        ));
        let address = EndpointAddr::new(host_id).with_ip_addr(ip);
        let invite = format!(
            "{}.{}",
            invite.rsplit_once('.').unwrap().0,
            EndpointTicket::new(address.clone())
        );
        let approval = async {
            timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(pairing) = host.status().pairing {
                        assert_eq!(
                            pairing.safety_code,
                            client.status().pairing.unwrap().safety_code
                        );
                        return host.confirm(pairing.peer_id, true);
                    }
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await?
        };
        let (paired, approved) = tokio::join!(client.pair(&invite), approval);
        approved?;
        assert_eq!(paired?, host_id);
        assert!(
            client.pair(&invite).await.is_err(),
            "an invitation must be single-use"
        );
        host.router.shutdown().await?;
        client.router.shutdown().await?;

        drop(host);
        drop(client);
        let host = Network::bind_with(host_key, host_file, lan()).await?;
        let address = EndpointAddr::new(host_id).with_ip_addr(SocketAddr::from((
            Ipv4Addr::LOCALHOST,
            host.router
                .endpoint()
                .bound_sockets()
                .into_iter()
                .find(|addr| addr.is_ipv4())
                .unwrap()
                .port(),
        )));
        let client = Network::bind_with(client_key.clone(), client_file, lan()).await?;
        assert_eq!(client.status().hosts, vec![host_id]);
        assert_eq!(host.status().clients, vec![client_key.public()]);
        {
            // Supply the new address in the isolated LAN test. Production lookups resolve it by key.
            let mut state = client.access.state.lock().unwrap();
            state.settings.hosts.insert(host_id, address.clone());
        }
        let unknown_file = sandbox.path().join("unknown.json");
        Settings {
            hosts: [(host_id, address)].into(),
            ..Settings::default()
        }
        .save(&unknown_file)?;
        let unknown = Network::bind_with(SecretKey::generate(), unknown_file, lan()).await?;
        assert!(unknown.files(host_id).await.is_err());

        let payload = vec![0x5a; 2 * 1024 * 1024 + 37];
        let source = sandbox.path().join("example.bin");
        let destination = sandbox.path().join("download.bin");
        tokio::fs::write(&source, &payload).await?;
        assert!(client.files(host_id).await?.is_empty());
        client.upload(host_id, source.clone()).await?;
        assert!(client.upload(host_id, source).await.is_err());
        let files = client.files(host_id).await?;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "example.bin");
        assert_eq!(files[0].size, payload.len() as u64);
        client
            .download(host_id, "example.bin".into(), destination.clone())
            .await?;
        assert_eq!(tokio::fs::read(destination).await?, payload);
        assert!(
            client
                .download(
                    host_id,
                    "../host.json".into(),
                    sandbox.path().join("escape")
                )
                .await
                .is_err()
        );

        host.revoke(client_key.public())?;
        assert!(client.files(host_id).await.is_err());
        unknown.router.shutdown().await?;
        client.router.shutdown().await?;
        host.router.shutdown().await?;
        Ok(())
    }

    fn lan() -> iroh::endpoint::Builder {
        Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
    }
}
