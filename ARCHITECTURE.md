# Personal Vault architecture

## Product

A Windows or Linux computer at home hosts a user-selected storage folder. Trusted clients can upload files, browse the stored file list and download files later, including when the client changes networks. The host must be awake, connected and running the app.

| Component | Responsibility |
| --- | --- |
| React and TypeScript | Host setup, pairing, trusted devices and file controls |
| Tauri | Desktop window, native file pickers and IPC |
| Rust and Tokio | Pairing, authorization, file operations and streaming |
| iroh | Public-key identity, QUIC with TLS, address lookup, NAT traversal and relay |
| System credential store | Persistent private Ed25519 key |

## Pairing and access

The host creates a random, single-use invitation that expires after five minutes. The invitation carries the host's public key and address hints. The two devices display a six-digit code bound to the invitation and both TLS identities. The host approves the authenticated peer and saves its public key in a local allowlist. The client stores the host key and address hints. Either app may restart without requiring another invitation. Revocation removes the key and closes active vault connections for that device.

By default, the client uses iroh to locate the host by its public key and connect directly or through the relay. It can also use the address hints from pairing. A relay forwards encrypted bytes. The host’s disk stores ordinary files.

## File operations

File requests require an authenticated, allowed key. The host opens the selected directory as a filesystem capability and accepts only validated top-level filenames. Directory entries that are links or reparse points are excluded. Each transfer uses bounded buffers and a BLAKE3 digest. Uploads are written to a staging file and published after integrity verification. If the intended destination already exists, the operation reports an error. The host's filesystem must support hard links for atomic, no-overwrite publication.

## Next development steps

Add subfolder navigation, transfer progress and resumable transfers. Add encryption for files stored on disk and a way to start the host automatically after login. Phone clients come after the desktop workflow. The earlier two-device room and chat are outside the home-storage workflow.
