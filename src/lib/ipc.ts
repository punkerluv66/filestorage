import { invoke } from "@tauri-apps/api/core";

export interface VaultFile {
  name: string;
  size: number;
}

export interface Status {
  endpointId: string;
  folder: string | null;
  relayConnected: boolean;
  inviting: boolean;
  pairing: { peerId: string; safetyCode: string; incoming: boolean } | null;
  clients: string[];
  hosts: string[];
  startupError: string | null;
}

export const vault = {
  status: () => invoke<Status>("vault_status"),
  chooseFolder: () => invoke<void>("choose_folder"),
  invite: () => invoke<string>("create_invite"),
  cancelInvite: () => invoke<void>("cancel_invite"),
  pair: (invite: string) => invoke<string>("pair_host", { invite }),
  confirm: (peerId: string, approved: boolean) => invoke<void>("confirm_pairing", { peerId, approved }),
  revoke: (peerId: string) => invoke<void>("revoke_client", { peerId }),
  files: (host: string) => invoke<VaultFile[]>("list_files", { host }),
  upload: (host: string) => invoke<boolean>("upload_file", { host }),
  download: (host: string, name: string) => invoke<boolean>("download_file", { host, name }),
};
