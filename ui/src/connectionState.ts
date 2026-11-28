import { createContext, useContext } from "react";

export type SelectedPeer = { device_id: string; display_name: string; os?: string | null };
export type Control = { paused: boolean; pairing_enabled: boolean; requested_peer_id?: string | null; connected_peer_id?: string | null; connected_peer_ids?: string[]; active_input_peer_id?: string | null; selected_peer?: SelectedPeer | null };
export type PeerSession = {
  device_id: string;
  display_name: string;
  os: string;
  input_active: boolean;
  send_audio: boolean;
  receive_audio: boolean;
  display_width: number;
  display_height: number;
  peer_side: import("./screenLayoutModel").PeerSide;
};

export function selectedPeerLabel(peer: SelectedPeer) {
  return [peer.display_name, peer.os, peer.device_id.slice(0, 8)].filter(Boolean).join(" · ");
}
type Controller = {
  control: Control | null;
  peers: PeerSession[];
  busy: boolean;
  error: string | null;
  change: (command: string, args: Record<string, unknown>) => Promise<boolean>;
};

// Keep context identity stable when the controls refresh during development.
export const ConnectionContext = createContext<Controller | null>(null);
export function useConnectionControl() {
  const value = useContext(ConnectionContext);
  if (!value) throw new Error("Connection controls require ConnectionProvider");
  return value;
}
