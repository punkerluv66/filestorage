# Personal Vault & Walkie-Talkie (`filestorage`)

A serverless, zero-trust peer-to-peer ecosystem combining a **Personal File Vault** (Home PC ⇄ Mobile/Remote Device) and an **Ephemeral P2P Walkie-Talkie Chat**, built with **Rust (`iroh` / QUIC)** and **TypeScript (React 19 + Tauri v2)**.

No accounts, no phone numbers, no central cloud storage. Device identity is a local **Ed25519** cryptographic keypair stored in the OS credential manager.

---

## Current Status (v0.1 — Core Vault Desktop MVP)

### Implemented
* **Cryptographic Device Identity (`src-tauri/src/identity.rs`):** Automatic generation and persistence of the `iroh` Ed25519 secret key in the native OS credential store (`keyring` — Windows Credential Manager / Linux Secret Service) with `zeroize` memory protection.
* **Zero-Trust Pairing Protocol (`/p2p/pair/1` in `src-tauri/src/net.rs`):**
  * Single-use, time-limited (5-minute) invitations (`vault1.<token>.<ticket>`).
  * Constant-time token verification (`subtle::ConstantTimeEq`).
  * 6-digit SAS (Short Authentication String) safety code derived via keyed `BLAKE3` over sorted peer identities for visual MITM verification.
  * Interactive approval/rejection on the host and instant client revocation.
* **Personal Vault Protocol (`/p2p/vault/1` in `src-tauri/src/vault.rs` & `transfer.rs`):**
  * Sandboxed directory access via `cap-std` (prevents path traversal, symlinks, and Windows reparse point escapes).
  * Atomic staged uploads (`.pv-upload-*` published via hard links) and non-clobbering downloads.
  * End-to-end stream integrity verification using `BLAKE3` digests.
  * Automatic connection racing (direct `EndpointId` discovery + cached `EndpointAddr` fallback) with `iroh` NAT traversal and relay fallback.
* **Desktop UI (`src/App.tsx`):** Clean React 19 + TypeScript interface for managing host storage folders, generating/accepting pairing invites, verifying 6-digit safety codes, revoking devices, and listing/uploading/downloading files.
* **Automated Tests & CI (`.github/workflows/build-desktop.yml`):** End-to-end integration test covering pairing, persistence across restarts, unauthorized access rejection, upload/list/download integrity, path-traversal prevention, and client revocation; GitHub Actions matrix for Windows (`.exe` NSIS) and Ubuntu (`.deb`).

### Roadmap (What's Next)
- [ ] **QR Code Pairing:** Render pairing invitations as QR codes on desktop and scan them via mobile camera.
- [ ] **Recursive Directory Tree & Chunked Resumable Transfers:** Support nested folders, file deletion/renaming, real-time transfer progress bars, and `offset`-based resume for interrupted transfers.
- [ ] **Module 2 — "Walkie-Talkie" (`/p2p/chat/1`):** Synchronous RAM-only ephemeral P2P chat between online peers, file sharing in chat, and low-latency Push-to-Talk voice over QUIC datagrams (Opus).
- [ ] **Role-Based ACL:** Distinguish between *Owner Devices* (full Vault + Chat access) and *Chat Contacts* (Chat-only access).
- [ ] **iOS / Mobile Target:** Initialize Tauri v2 iOS target (`cargo tauri ios init`), mobile-responsive navigation, photo gallery backup, and macOS GitHub Actions `.ipa` build pipeline.

---

## Architecture Overview

| Layer | Technology | Purpose |
| :--- | :--- | :--- |
| **App Shell** | **Tauri v2** | Lightweight cross-platform runtime bridging TypeScript UI and Rust core. |
| **Frontend** | **React 19 + TypeScript + Vite** | Reactive user interface for Vault management and pairing workflows. |
| **P2P Networking** | **`iroh` (QUIC)** | Public-key addressing (`EndpointId`), UDP hole punching, mDNS, DHT, and E2EE relays. |
| **Filesystem Sandbox** | **`cap-std` + `cap-fs-ext`** | Capability-based directory sandboxing against symlink/traversal attacks. |
| **Cryptography** | **`blake3`, `keyring`, `subtle`, `zeroize`** | Integrity verification, OS keychain storage, constant-time checks, and memory wiping. |

For a deep dive into the system design and protocols (in Russian), see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Getting Started

### Prerequisites
* **Node.js** 24+ and **npm**
* **Rust** stable (`1.91+`, 2024 edition)
* **Windows:** MSVC Build Tools & WebView2
* **Ubuntu / Debian:**
  ```sh
  sudo apt-get update
  sudo apt-get install -y build-essential pkg-config libwebkit2gtk-4.1-dev libssl-dev librsvg2-dev
  ```

### Development
```sh
npm ci
npm run tauri dev
```

### Testing & Production Build
```sh
# Type-check and build frontend
npm run build

# Run Rust unit & P2P integration tests
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib

# Build native desktop installer (.exe / .deb)
npm run tauri -- build
```

---

## How to Pair Two Devices

1. **On the Storage Host:** Open the app, click **Choose storage folder**, and select a folder on an NTFS/ext4 drive.
2. Click **Invite a device** and copy the generated one-time invitation string.
3. **On the Client Device:** Paste the invitation into **Connect to home storage** and click **Pair with storage**.
4. **Verify & Confirm:** Compare the 6-digit safety code displayed on both screens, then click **Confirm pairing** on the host.
5. **Transfer Files:** Use **Upload file**, **Refresh files**, and **Download** under the **Stored files** section.
