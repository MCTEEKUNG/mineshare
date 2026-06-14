/* Shared types — moved out of App.tsx so pages can import them. */

export type Status = {
  peer_connected: boolean;
  peer_addr: string | null;
  peer_name: string | null;
  sent_pkts: number;
  recv_pkts: number;
  injected: number;
  audio_recv: number;
  inject_errs: number;
  decrypt_errs: number;
  local_in_remote: boolean;
  peer_in_remote: boolean;
  input_locked: boolean;
  game_drive: "off" | "driving" | "receiving";
  anticheat_warning: string | null;
  keys_forwarded: number;
  keys_injected: number;
  keyboard_target: "smart" | "auto" | "force_peer" | "force_local";
  local_version: string;
  peer_version: string | null;
};

export type Latency = {
  samples: number;
  last_ms: number | null;
  min_ms: number | null;
  avg_ms: number | null;
  p50_ms: number | null;
  p95_ms: number | null;
  max_ms: number | null;
  histogram: number[];
  bin_edges_ms: number[];
};

export type Transfer = {
  id: number;
  direction: "sending" | "receiving";
  status: "pending" | "active" | "verifying" | "done" | "cancelled" | "failed";
  name: string;
  size_bytes: number;
  bytes_so_far: number;
  final_path: string | null;
  error: string | null;
  seconds_elapsed: number;
};

export type ToastEntry = {
  id: number;
  direction: "sending" | "receiving";
  status: "done" | "failed" | "cancelled";
  name: string;
  final_path: string | null;
};
