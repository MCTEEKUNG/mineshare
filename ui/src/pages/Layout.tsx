import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type PeerSide = "left" | "right";

type Layout = {
  peer_side: PeerSide;
};

type Status = {
  peer_connected: boolean;
  peer_addr: string | null;
};

/**
 * Layout editor — Slice 2a.
 *
 * Renders two scaled tiles representing the local + peer monitors,
 * arranged according to the persisted `peer_side`. A toggle flips
 * between "peer left" and "peer right" and round-trips the choice
 * through the Tauri `set_layout` command, which persists to
 * `<config_dir>/MineShare/layout.json` and pushes the new side to
 * `mineshare-input`.
 *
 * Real free-form drag-and-drop with snap-to-edge geometry lands in
 * Slice 2b along with the input-module conditional logic that
 * actually flips edge detection / virt_x / boundary warp based on
 * the side. Until then the JSON config persists but the bridge
 * keeps using its M0–M4 hardcoded convention.
 */
export default function LayoutPage() {
  const [layout, setLayout] = useState<Layout | null>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const [pending, setPending] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    invoke<Layout>("get_layout").then(setLayout).catch((e) => setErr(String(e)));
    const tick = () => invoke<Status>("get_status").then(setStatus).catch(() => {});
    tick();
    const id = setInterval(tick, 1500);
    return () => clearInterval(id);
  }, []);

  async function swap() {
    if (!layout) return;
    const next: Layout = {
      peer_side: layout.peer_side === "right" ? "left" : "right",
    };
    setPending(true);
    setErr(null);
    try {
      await invoke("set_layout", { cfg: next });
      setLayout(next);
    } catch (e) {
      setErr(String(e));
    } finally {
      setPending(false);
    }
  }

  if (!layout) {
    return (
      <p className="text-sm text-neutral-400">
        {err ? `failed to load layout: ${err}` : "loading layout…"}
      </p>
    );
  }

  return (
    <section>
      <p className="text-sm text-neutral-500 mb-6 max-w-prose">
        Choose which side of <em>this</em> machine the peer monitor
        is "stuck to". Edge detection, cursor warps, and Remote-mode
        sign conventions all flip when you toggle the side.
      </p>

      <div className="rounded-lg border border-neutral-200 dark:border-neutral-800 p-8 mb-6">
        <LayoutCanvas peerSide={layout.peer_side} status={status} />
      </div>

      <div className="flex items-center justify-between">
        <p className="text-sm">
          <span className="text-neutral-500">Peer is on the </span>
          <span className="font-semibold">
            {layout.peer_side === "right" ? "right" : "left"}
          </span>
          <span className="text-neutral-500"> of this display.</span>
        </p>
        <button
          onClick={swap}
          disabled={pending}
          className="rounded-md border border-neutral-300 dark:border-neutral-700 px-4 py-2 text-sm font-medium hover:bg-neutral-50 dark:hover:bg-neutral-900 disabled:opacity-50"
        >
          {pending ? "Saving…" : "Swap sides"}
        </button>
      </div>
      {err ? <p className="text-xs text-red-600 mt-3">{err}</p> : null}
    </section>
  );
}

function LayoutCanvas({
  peerSide,
  status,
}: {
  peerSide: PeerSide;
  status: Status | null;
}) {
  // Cosmetic-only widths — we don't have the peer's real screen
  // size in the layout config (the daemon learns it at handshake
  // time and Slice 2b will surface it here).
  const SELF_W = 240;
  const SELF_H = 150;
  const PEER_W = 200;
  const PEER_H = 125;
  const GAP = 12;
  const peerLeft = peerSide === "left" ? 0 : SELF_W + GAP;
  const selfLeft = peerSide === "left" ? PEER_W + GAP : 0;
  const totalW = SELF_W + PEER_W + GAP;
  const totalH = Math.max(SELF_H, PEER_H);

  return (
    <div
      className="relative mx-auto"
      style={{ width: totalW, height: totalH }}
    >
      <Tile
        label="this machine"
        sub="local"
        x={selfLeft}
        y={(totalH - SELF_H) / 2}
        w={SELF_W}
        h={SELF_H}
        accent
      />
      <Tile
        label={status?.peer_addr ? "peer" : "no peer"}
        sub={status?.peer_addr ?? "—"}
        x={peerLeft}
        y={(totalH - PEER_H) / 2}
        w={PEER_W}
        h={PEER_H}
        muted={!status?.peer_connected}
      />
    </div>
  );
}

function Tile({
  label,
  sub,
  x,
  y,
  w,
  h,
  accent,
  muted,
}: {
  label: string;
  sub: string;
  x: number;
  y: number;
  w: number;
  h: number;
  accent?: boolean;
  muted?: boolean;
}) {
  return (
    <div
      className={
        "absolute rounded-md border-2 flex flex-col items-center justify-center select-none transition-colors " +
        (accent
          ? "border-emerald-500 bg-emerald-50 dark:bg-emerald-950/30"
          : muted
            ? "border-dashed border-neutral-300 dark:border-neutral-700 bg-neutral-50 dark:bg-neutral-900/40 text-neutral-400"
            : "border-neutral-400 bg-neutral-50 dark:bg-neutral-900")
      }
      style={{ left: x, top: y, width: w, height: h }}
    >
      <p className="text-sm font-medium">{label}</p>
      <p className={"text-[11px] " + (accent ? "text-emerald-700" : "text-neutral-500")}>
        {sub}
      </p>
    </div>
  );
}
