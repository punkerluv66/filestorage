import { useEffect, useState } from "react";
import { vault, type Status, type VaultFile } from "./lib/ipc";

const number = new Intl.NumberFormat("en", { maximumFractionDigits: 1 });

function formatSize(bytes: number) {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let unit = 0;
  while (bytes >= 1024 && unit < units.length - 1) {
    bytes /= 1024;
    unit += 1;
  }
  return `${number.format(bytes)} ${units[unit]}`;
}

export function App() {
  const [status, setStatus] = useState<Status>();
  const [startupError, setStartupError] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [invite, setInvite] = useState("");
  const [joinCode, setJoinCode] = useState("");
  const [selectedHost, setSelectedHost] = useState("");
  const [listing, setListing] = useState<{ host: string; files: VaultFile[] }>();
  const [busy, setBusy] = useState("");
  const host = selectedHost || status?.hosts[0] || "";

  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const next = await vault.status();
        if (!active) return;
        setStatus(next);
        setStartupError("");
      } catch (problem) {
        if (!active) return;
        setStartupError(String(problem));
      }
      timer = setTimeout(refresh, 700);
    }
    void refresh();
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, []);

  async function run(label: string, action: () => Promise<void>) {
    setBusy(label);
    setError("");
    setNotice("");
    try {
      await action();
      setStatus(await vault.status());
    } catch (problem) {
      setError(String(problem));
    } finally {
      setBusy("");
    }
  }

  async function refreshFiles(id: string) {
    setListing({ host: id, files: await vault.files(id) });
  }

  const pairing = status?.pairing;
  const files = listing?.host === host ? listing.files : undefined;
  const waiting = Boolean(busy);
  const ready = Boolean(status && !startupError);

  return (
    <>
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true"><span /></span>
          Personal Vault
        </div>
        <span className={ready ? "status good" : "status"} role="status">
          <span className="dot" />
          {startupError ? "Startup failed" : status?.relayConnected ? "Relay available" : ready ? "Node ready" : "Starting"}
        </span>
      </header>
      <main>
        <h1>Personal Vault</h1>
        <p className="intro">Store files on your home computer and access them from your paired devices.</p>

        {pairing && (
          <section className="panel device action">
            <h2>{pairing.incoming ? "Confirm this device" : "Waiting for the storage computer"}</h2>
            <p>Compare this code on both devices before confirming.</p>
            <div className="safety-code mono">{pairing.safetyCode}</div>
            <label htmlFor="pairing-key">Other device’s public key</label>
            <input id="pairing-key" className="mono" readOnly value={pairing.peerId} />
            {pairing.incoming && (
              <div className="buttons">
                <button disabled={waiting} onClick={() => void run("Confirming…", () => vault.confirm(pairing.peerId, true))}>Confirm pairing</button>
                <button className="secondary" disabled={waiting} onClick={() => void run("Rejecting…", () => vault.confirm(pairing.peerId, false))}>Reject</button>
              </div>
            )}
          </section>
        )}

        <div className="columns">
          <section className="panel action">
            <h2>Host storage on this computer</h2>
            <p>Keep the app running on the computer that stores your files.</p>
            {status?.folder && <p className="mono wrap">{status.folder}</p>}
            <button className="secondary" disabled={!ready || waiting || Boolean(pairing)} onClick={() => void run("Choosing folder…", vault.chooseFolder)}>
              {status?.folder ? "Change storage folder" : "Choose storage folder"}
            </button>
            {status?.startupError && <p className="error" role="alert">{status.startupError}</p>}
            {status?.folder && !status.startupError && !pairing && (
              status.inviting && invite ? (
                <div className="feedback">
                  <label htmlFor="invitation">One-time invitation</label>
                  <input id="invitation" className="mono" readOnly value={invite} onFocus={(event) => event.target.select()} />
                  <div className="buttons">
                    <button disabled={waiting} onClick={() => void run("Copying…", async () => {
                      await navigator.clipboard.writeText(invite);
                      setNotice("Invitation copied");
                    })}>Copy invitation</button>
                    <button className="secondary" disabled={waiting} onClick={() => void run("Cancelling…", vault.cancelInvite)}>Cancel invitation</button>
                  </div>
                </div>
              ) : (
                <div className="buttons">
                  <button disabled={waiting} onClick={() => void run("Creating invitation…", async () => setInvite(await vault.invite()))}>Invite a device</button>
                </div>
              )
            )}
            {Boolean(status?.clients.length) && (
              <ul className="files clients" aria-label="Trusted devices">
                {status?.clients.map((id) => (
                  <li key={id}>
                    <span className="mono wrap">{id}</span>
                    <button className="secondary" disabled={waiting} onClick={() => void run("Revoking access…", () => vault.revoke(id))}>Revoke</button>
                  </li>
                ))}
              </ul>
            )}
          </section>
          <section className="panel action">
            <h2>Connect to home storage</h2>
            <form onSubmit={(event) => {
              event.preventDefault();
              void run("Pairing…", async () => {
                const id = await vault.pair(joinCode);
                setSelectedHost(id);
                setJoinCode("");
                await refreshFiles(id);
              });
            }}>
              <label htmlFor="join-code">Invitation from your storage computer</label>
              <input id="join-code" className="mono" value={joinCode} maxLength={4096} disabled={waiting || Boolean(pairing)}
                autoComplete="off" spellCheck={false} onChange={(event) => setJoinCode(event.target.value)} />
              <button disabled={!ready || waiting || Boolean(pairing) || !joinCode.trim()}>Pair with storage</button>
            </form>
          </section>
        </div>

        {host && (
          <section className="panel files-panel">
            <div className="device-top">
              <h2>Stored files</h2>
              <div className="buttons compact">
                <button className="secondary" disabled={waiting} onClick={() => void run("Loading files…", () => refreshFiles(host))}>Refresh files</button>
                <button disabled={waiting} onClick={() => void run("Uploading…", async () => {
                  if (await vault.upload(host)) {
                    setNotice("File uploaded and verified");
                    await refreshFiles(host);
                  }
                })}>Upload file</button>
              </div>
            </div>
            <label htmlFor="host">Storage computer</label>
            <select id="host" className="mono" value={host} disabled={waiting} onChange={(event) => {
              const id = event.target.value;
              setSelectedHost(id);
              void run("Loading files…", () => refreshFiles(id));
            }}>
              {status?.hosts.map((id) => <option key={id} value={id}>{id}</option>)}
            </select>
            {files?.length ? (
              <ul className="files" aria-label="Stored files">
                {files.map((file) => (
                  <li key={file.name}>
                    <div className="wrap"><strong>{file.name}</strong><div className="muted">{formatSize(file.size)}</div></div>
                    <button className="secondary" disabled={waiting} onClick={() => void run("Downloading…", async () => {
                      if (await vault.download(host, file.name)) setNotice("File downloaded and verified");
                    })}>Download</button>
                  </li>
                ))}
              </ul>
            ) : <p className="muted feedback">{files ? "The storage folder is empty." : "Refresh files to load the storage folder."}</p>}
          </section>
        )}

        {(busy || notice) && <p className="feedback muted" role="status">{busy || notice}</p>}
        {(startupError || error) && <p className="error" role="alert">{startupError || error}</p>}
      </main>
    </>
  );
}
