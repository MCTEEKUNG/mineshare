import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type PeerSide = "left" | "right" | "top" | "bottom";
type Layout = { peer_side: PeerSide };
type Status = { peer_connected: boolean; peer_addr: string | null };

/**
 * Display Settings-style layout editor.
 *
 * Renders a draggable peer-monitor tile around a fixed local-monitor
 * tile. While the user drags, the peer tile floats free under the
 * cursor; on release we project the offset onto the dominant axis
 * (whichever of `|dx|` and `|dy|` is larger) and snap the peer to
 * that side of the local tile. The resulting `PeerSide` round-trips
 * through `set_layout`, which persists to disk and pushes the new
 * side to `mineshare-input` so the bridge picks it up immediately.
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

  async function setSide(next: PeerSide) {
    if (!layout || layout.peer_side === next) return;
    setPending(true);
    setErr(null);
    try {
      await invoke("set_layout", { cfg: { peer_side: next } });
      setLayout({ peer_side: next });
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
        Drag the peer monitor against any edge of <em>this</em>{" "}
        machine's display to set the bridge direction. Edge
        detection, cursor warps, and Remote-mode sign conventions
        all flip when you change the side. Auto-saves on drop.
      </p>

      <DragCanvas
        side={layout.peer_side}
        status={status}
        disabled={pending}
        onChoose={setSide}
      />

      <div className="mt-6 flex items-center justify-between text-sm">
        <p>
          <span className="text-neutral-500">Peer is on the </span>
          <span className="font-semibold">{layout.peer_side}</span>
          <span className="text-neutral-500"> of this display.</span>
        </p>
        {pending ? <span className="text-xs text-neutral-400">saving…</span> : null}
      </div>
      {err ? <p className="text-xs text-red-600 mt-3">{err}</p> : null}
    </section>
  );
}

/* ------------------------------------------------------------------ */

// Canvas large enough to hold the local tile centred + the peer
// tile flush against any of the four edges with breathing room.
// Required width  = LOCAL_W + 2*GAP + 2*PEER_W + margins
// Required height = LOCAL_H + 2*GAP + 2*PEER_H + margins
const LOCAL_W = 200;
const LOCAL_H = 125;
const PEER_W = 170;
const PEER_H = 106;
const GAP = 16;
const MARGIN = 24;
const CANVAS_W = LOCAL_W + 2 * GAP + 2 * PEER_W + 2 * MARGIN; // 580
const CANVAS_H = LOCAL_H + 2 * GAP + 2 * PEER_H + 2 * MARGIN; // 417
/** Pixels of free-drag distance from the local tile's center
 *  beyond which we treat the gesture as "moving toward an edge". */
const SNAP_THRESHOLD = 40;

function DragCanvas({
  side,
  status,
  disabled,
  onChoose,
}: {
  side: PeerSide;
  status: Status | null;
  disabled?: boolean;
  onChoose: (side: PeerSide) => void;
}) {
  const localCx = CANVAS_W / 2;
  const localCy = CANVAS_H / 2;
  const localBox = {
    x: localCx - LOCAL_W / 2,
    y: localCy - LOCAL_H / 2,
    w: LOCAL_W,
    h: LOCAL_H,
  };

  // Resting peer-tile position derived from the configured side.
  function restingPeerCenter(s: PeerSide): { cx: number; cy: number } {
    switch (s) {
      case "left":
        return { cx: localBox.x - GAP - PEER_W / 2, cy: localCy };
      case "right":
        return { cx: localBox.x + localBox.w + GAP + PEER_W / 2, cy: localCy };
      case "top":
        return { cx: localCx, cy: localBox.y - GAP - PEER_H / 2 };
      case "bottom":
        return { cx: localCx, cy: localBox.y + localBox.h + GAP + PEER_H / 2 };
    }
  }

  const [center, setCenter] = useState(restingPeerCenter(side));
  const [hover, setHover] = useState<PeerSide | null>(null);
  const dragRef = useRef<{ startMx: number; startMy: number; startCx: number; startCy: number } | null>(null);
  const canvasRef = useRef<HTMLDivElement | null>(null);

  // Snap position when the persisted side changes (e.g. peer-side
  // saved by the other tab → coming back here).
  useEffect(() => {
    if (!dragRef.current) setCenter(restingPeerCenter(side));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [side]);

  function projectToSide(cx: number, cy: number): PeerSide | null {
    const dx = cx - localCx;
    const dy = cy - localCy;
    if (Math.abs(dx) < SNAP_THRESHOLD && Math.abs(dy) < SNAP_THRESHOLD) return null;
    if (Math.abs(dx) >= Math.abs(dy)) return dx > 0 ? "right" : "left";
    return dy > 0 ? "bottom" : "top";
  }

  function onPointerDown(e: React.PointerEvent) {
    if (disabled) return;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    dragRef.current = {
      startMx: e.clientX,
      startMy: e.clientY,
      startCx: center.cx,
      startCy: center.cy,
    };
  }

  function onPointerMove(e: React.PointerEvent) {
    if (!dragRef.current) return;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;
    const nx = dragRef.current.startCx + (e.clientX - dragRef.current.startMx);
    const ny = dragRef.current.startCy + (e.clientY - dragRef.current.startMy);
    setCenter({
      cx: clamp(nx, PEER_W / 2, CANVAS_W - PEER_W / 2),
      cy: clamp(ny, PEER_H / 2, CANVAS_H - PEER_H / 2),
    });
    setHover(projectToSide(nx, ny));
  }

  function onPointerUp() {
    if (!dragRef.current) return;
    const projected = projectToSide(center.cx, center.cy);
    dragRef.current = null;
    setHover(null);
    if (projected && projected !== side) {
      onChoose(projected);
      setCenter(restingPeerCenter(projected));
    } else {
      // Snap back if drag didn't clear the threshold or chose
      // the already-active side.
      setCenter(restingPeerCenter(side));
    }
  }

  return (
    <div
      ref={canvasRef}
      className="relative mx-auto rounded-lg border border-neutral-200 dark:border-neutral-800 bg-neutral-50 dark:bg-neutral-950 select-none"
      style={{ width: CANVAS_W, height: CANVAS_H }}
    >
      {/* Drop-zone hints — emphasise the side the user is hovering. */}
      <EdgeHint visible={hover === "left"} side="left" box={localBox} />
      <EdgeHint visible={hover === "right"} side="right" box={localBox} />
      <EdgeHint visible={hover === "top"} side="top" box={localBox} />
      <EdgeHint visible={hover === "bottom"} side="bottom" box={localBox} />

      <Tile
        x={localBox.x}
        y={localBox.y}
        w={localBox.w}
        h={localBox.h}
        label="this machine"
        sub="local"
        accent
      />

      <div
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        className={
          "absolute touch-none " +
          (disabled ? "cursor-not-allowed opacity-60" : "cursor-grab active:cursor-grabbing")
        }
        style={{
          left: center.cx - PEER_W / 2,
          top: center.cy - PEER_H / 2,
          width: PEER_W,
          height: PEER_H,
        }}
      >
        <Tile
          x={0}
          y={0}
          w={PEER_W}
          h={PEER_H}
          label={status?.peer_addr ? "peer" : "no peer"}
          sub={status?.peer_addr ?? "—"}
          muted={!status?.peer_connected}
          relative
        />
      </div>
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
  relative,
}: {
  label: string;
  sub: string;
  x: number;
  y: number;
  w: number;
  h: number;
  accent?: boolean;
  muted?: boolean;
  relative?: boolean;
}) {
  return (
    <div
      className={
        (relative ? "relative " : "absolute ") +
        "rounded-md border-2 flex flex-col items-center justify-center pointer-events-none select-none transition-colors " +
        (accent
          ? "border-emerald-500 bg-emerald-50 dark:bg-emerald-950/40"
          : muted
            ? "border-dashed border-neutral-300 dark:border-neutral-700 bg-neutral-50 dark:bg-neutral-900/40 text-neutral-400"
            : "border-neutral-400 bg-white dark:bg-neutral-900")
      }
      style={
        relative ? { width: w, height: h } : { left: x, top: y, width: w, height: h }
      }
    >
      <p className="text-sm font-medium">{label}</p>
      <p className={"text-[11px] " + (accent ? "text-emerald-700" : "text-neutral-500")}>
        {sub}
      </p>
    </div>
  );
}

function EdgeHint({
  visible,
  side,
  box,
}: {
  visible: boolean;
  side: PeerSide;
  box: { x: number; y: number; w: number; h: number };
}) {
  if (!visible) return null;
  const T = 6;
  let style: React.CSSProperties;
  switch (side) {
    case "left":
      style = { left: box.x - T, top: box.y, width: T, height: box.h };
      break;
    case "right":
      style = { left: box.x + box.w, top: box.y, width: T, height: box.h };
      break;
    case "top":
      style = { left: box.x, top: box.y - T, width: box.w, height: T };
      break;
    case "bottom":
      style = { left: box.x, top: box.y + box.h, width: box.w, height: T };
      break;
  }
  return (
    <div
      className="absolute rounded-full bg-emerald-400/80 pointer-events-none transition-opacity"
      style={style}
    />
  );
}

function clamp(n: number, lo: number, hi: number) {
  return Math.max(lo, Math.min(hi, n));
}
