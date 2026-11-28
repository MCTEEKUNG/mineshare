import { useRef, useState, type CSSProperties, type PointerEvent } from "react";
import { useConnectionControl, type PeerSession } from "../connectionState";
import { useT } from "../i18n";
import { screenSides, type PeerSide } from "../screenLayoutModel";
import type { Status } from "../types";
import LayoutPage from "../pages/Layout";

type Position = { x: number; y: number };
const positions: Record<PeerSide, Position> = {
  left: { x: 17, y: 50 }, right: { x: 83, y: 50 }, top: { x: 50, y: 18 }, bottom: { x: 50, y: 82 },
};
/** Which edge of this computer a dragged point is nearest to, or null while it is still over the computer. */
function dropSide(point: Position): PeerSide | null {
  const dx = point.x - 50, dy = point.y - 50;
  if (Math.max(Math.abs(dx), Math.abs(dy)) < 16) return null;
  return Math.abs(dx) > Math.abs(dy) ? dx < 0 ? "left" : "right" : dy < 0 ? "top" : "bottom";
}
/** Distance between tiles that share an edge, in canvas percent; tiles shrink to fit it so they never overlap. */
const groupSpacing = (count: number) => Math.min(32, 84 / count);
function devicePosition(peer: PeerSession, peers: PeerSession[]): Position {
  const group = peers.filter(other => other.peer_side === peer.peer_side);
  const offset = (group.findIndex(other => other.device_id === peer.device_id) - (group.length - 1) / 2) * groupSpacing(group.length);
  const position = positions[peer.peer_side];
  return peer.peer_side === "left" || peer.peer_side === "right" ? { x: position.x, y: position.y + offset } : { x: position.x + offset, y: position.y };
}

export default function WorkspaceLayout({ peerName, connected, status }: { peerName?: string; connected?: boolean; status?: Status | null }) {
  const { peers } = useConnectionControl();
  return peers.length ? <DeviceLayout peers={peers} /> : <LayoutPage peerName={peerName} connected={connected} status={status} />;
}

function DeviceLayout({ peers }: { peers: PeerSession[] }) {
  const { t } = useT();
  const { busy, change } = useConnectionControl();
  const canvas = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<{ deviceId: string; pointerId: number; point: Position } | null>(null);
  const [error, setError] = useState(false);
  const peerCount = Math.max(1, ...screenSides.map(side => peers.filter(peer => peer.peer_side === side).length));
  const tileWidth = Math.min(27, 60 / peerCount);
  const activePeer = peers.find(peer => peer.input_active);

  async function place(peer: PeerSession, side: PeerSide) {
    if (busy || peer.peer_side === side) return;
    // One device per edge: the crossing edge must name exactly one device, so a device already
    // there trades places with the one being moved.
    const occupant = peers.find(other => other.device_id !== peer.device_id && other.peer_side === side);
    if (occupant && !await change("set_peer_layout", { deviceId: occupant.device_id, side: peer.peer_side })) { setError(true); return; }
    setError(!await change("set_peer_layout", { deviceId: peer.device_id, side }));
  }
  function move(event: PointerEvent<HTMLButtonElement>) {
    if (!drag || drag.pointerId !== event.pointerId) return;
    const bounds = canvas.current?.getBoundingClientRect();
    if (bounds) setDrag({ ...drag, point: { x: Math.max(5, Math.min(95, (event.clientX - bounds.left) / bounds.width * 100)), y: Math.max(5, Math.min(95, (event.clientY - bounds.top) / bounds.height * 100)) } });
  }
  function finish(event: PointerEvent<HTMLButtonElement>, peer: PeerSession) {
    if (!drag || drag.pointerId !== event.pointerId) return;
    const point = drag.point;
    setDrag(null);
    event.currentTarget.releasePointerCapture(event.pointerId);
    const side = dropSide(point);
    if (side) void place(peer, side);
  }

  return <div>
    <p className="text-xs leading-relaxed text-ds-text-muted mb-4" role={activePeer ? "status" : undefined}>
      {activePeer ? t(`peers_control_${activePeer.peer_side}`).replace("{device}", activePeer.display_name) : t("peers_layout_hint")}
    </p>
    <div ref={canvas} className="layout-canvas" style={{ height: Math.max(310, peerCount * 105) }} role="group" aria-label={t("settings_screens")}>
      {drag && screenSides.map(side => <div key={side} className={"display-drop-slot " + (dropSide(drag.point) === side ? "is-target" : "")} aria-hidden="true"
        style={{ left: `${positions[side].x}%`, top: `${positions[side].y}%`, width: `${tileWidth}%`, aspectRatio: "16/10", maxHeight: "28%" }}>{t(`layout_${side}`)}</div>)}
      <div className="display-tile display-local" style={{ left: "50%", top: "50%", width: "27%", aspectRatio: "16/10" }}><span className="display-name">{t("workspace_local")}</span></div>
      {peers.map(peer => {
        const point = drag?.deviceId === peer.device_id ? drag.point : devicePosition(peer, peers);
        const ratio = peer.display_width > 0 && peer.display_height > 0 ? peer.display_width / peer.display_height : peer.os.toLowerCase().includes("android") ? 9 / 19.5 : 16 / 10;
        const phone = ratio < 1;
        const style: CSSProperties = { left: `${point.x}%`, top: `${point.y}%`, width: `${phone ? tileWidth * .58 : tileWidth}%`, aspectRatio: String(Math.max(.4, Math.min(2.5, ratio))), maxHeight: `${Math.min(28, groupSpacing(peers.filter(other => other.peer_side === peer.peer_side).length) - 3)}%` };
        return <button key={peer.device_id} className={"display-tile display-peer " + (drag?.deviceId === peer.device_id ? "is-dragging" : "")}
          style={style} disabled={busy} aria-label={`${t("layout_drag_screen")}: ${peer.display_name}`} data-peer-side={peer.peer_side} data-input-active={peer.input_active}
          title={`${peer.display_name} · ${t(`layout_${peer.peer_side}`)}`} aria-keyshortcuts="ArrowUp ArrowDown ArrowLeft ArrowRight"
          onPointerDown={event => {
            if (event.button !== 0 || busy) return;
            event.preventDefault(); event.currentTarget.focus(); event.currentTarget.setPointerCapture(event.pointerId);
            setDrag({ deviceId: peer.device_id, pointerId: event.pointerId, point });
          }} onPointerMove={move} onPointerUp={event => finish(event, peer)} onPointerCancel={() => setDrag(null)} onLostPointerCapture={() => setDrag(null)}
          onKeyDown={event => {
            if (event.key === "Escape") { setDrag(null); return; }
            const side = ({ ArrowUp: "top", ArrowDown: "bottom", ArrowLeft: "left", ArrowRight: "right" } as Record<string, PeerSide>)[event.key];
            if (side) { event.preventDefault(); if (!event.repeat) void place(peer, side); }
          }}>
          <span className="display-name">{peer.display_name}</span>
          {peer.input_active && <span className="sr-only">{t("peers_active")}</span>}
        </button>;
      })}
    </div>
    {activePeer && <p className="text-xs leading-relaxed text-ds-text-muted mt-3">{t("peers_layout_hint")}</p>}
    {error && <p className="text-sm text-ds-danger mt-3" role="alert">{t("layout_save_failed")}</p>}
  </div>;
}
