mod identity;
mod net;
mod settings;
mod transfer;
mod vault;
mod wire;

use iroh::EndpointId;
use net::{Network, Status};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder, FilePath};
use tokio::sync::{OnceCell, oneshot};
use vault::VaultFile;

struct NetworkState(OnceCell<Network>);
type CommandResult<T> = Result<T, String>;

fn message(error: anyhow::Error) -> String {
    format!("{error:#}")
}

impl NetworkState {
    async fn get(&self, app: &AppHandle) -> CommandResult<&Network> {
        self.0
            .get_or_try_init(|| async {
                let path = app.path().app_local_data_dir()?.join("vault.json");
                let key = tokio::task::spawn_blocking(identity::load_or_create).await??;
                Network::bind(key, path).await
            })
            .await
            .map_err(message)
    }
}

#[tauri::command]
async fn vault_status(app: AppHandle, state: State<'_, NetworkState>) -> CommandResult<Status> {
    Ok(state.get(&app).await?.status())
}

#[tauri::command]
async fn choose_folder(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, NetworkState>,
) -> CommandResult<()> {
    let network = state.get(&app).await?;
    let dialog = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("Choose your storage folder");
    if let Some(path) = pick_path(dialog, FileDialogBuilder::pick_folder).await? {
        network.set_folder(path).map_err(message)?;
    }
    Ok(())
}

#[tauri::command]
async fn create_invite(app: AppHandle, state: State<'_, NetworkState>) -> CommandResult<String> {
    state.get(&app).await?.create_invite().map_err(message)
}

#[tauri::command]
async fn cancel_invite(app: AppHandle, state: State<'_, NetworkState>) -> CommandResult<()> {
    state.get(&app).await?.cancel_invite();
    Ok(())
}

#[tauri::command]
async fn pair_host(
    app: AppHandle,
    state: State<'_, NetworkState>,
    invite: String,
) -> CommandResult<EndpointId> {
    state.get(&app).await?.pair(&invite).await.map_err(message)
}

#[tauri::command]
async fn confirm_pairing(
    app: AppHandle,
    state: State<'_, NetworkState>,
    peer_id: EndpointId,
    approved: bool,
) -> CommandResult<()> {
    state
        .get(&app)
        .await?
        .confirm(peer_id, approved)
        .map_err(message)
}

#[tauri::command]
async fn revoke_client(
    app: AppHandle,
    state: State<'_, NetworkState>,
    peer_id: EndpointId,
) -> CommandResult<()> {
    state.get(&app).await?.revoke(peer_id).map_err(message)
}

#[tauri::command]
async fn list_files(
    app: AppHandle,
    state: State<'_, NetworkState>,
    host: EndpointId,
) -> CommandResult<Vec<VaultFile>> {
    state.get(&app).await?.files(host).await.map_err(message)
}

#[tauri::command]
async fn upload_file(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, NetworkState>,
    host: EndpointId,
) -> CommandResult<bool> {
    let network = state.get(&app).await?;
    let dialog = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("Choose a file to upload");
    let Some(path) = pick_path(dialog, FileDialogBuilder::pick_file).await? else {
        return Ok(false);
    };
    network.upload(host, path).await.map_err(message)?;
    Ok(true)
}

#[tauri::command]
async fn download_file(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, NetworkState>,
    host: EndpointId,
    name: String,
) -> CommandResult<bool> {
    transfer::validate_name(&name).map_err(message)?;
    let network = state.get(&app).await?;
    let dialog = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("Save downloaded file")
        .set_file_name(&name);
    let Some(path) = pick_path(dialog, FileDialogBuilder::save_file).await? else {
        return Ok(false);
    };
    network.download(host, name, path).await.map_err(message)?;
    Ok(true)
}

type DialogCallback = Box<dyn FnOnce(Option<FilePath>) + Send + 'static>;

async fn pick_path(
    dialog: FileDialogBuilder<tauri::Wry>,
    open: impl FnOnce(FileDialogBuilder<tauri::Wry>, DialogCallback),
) -> CommandResult<Option<PathBuf>> {
    let (send, recv) = oneshot::channel();
    open(
        dialog,
        Box::new(move |path| {
            let _ = send.send(path);
        }),
    );
    recv.await
        .map_err(|_| "File dialog closed unexpectedly".to_owned())?
        .map(FilePath::into_path)
        .transpose()
        .map_err(|error| error.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(NetworkState(OnceCell::new()))
        .invoke_handler(tauri::generate_handler![
            vault_status,
            choose_folder,
            create_invite,
            cancel_invite,
            pair_host,
            confirm_pairing,
            revoke_client,
            list_files,
            upload_file,
            download_file,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Filestorage");
}
