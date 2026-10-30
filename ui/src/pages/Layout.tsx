import { useEffect, useId, useLayoutEffect, useRef, useState, type PointerEvent, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "../i18n";
import type { Status } from "../types";
import { screenDropSide, screenGeometry, screenPosition, screenSides, type PeerSide, type Point } from "../screenLayoutModel";

type Layout = { peer_side: PeerSide };
type Drag = { center: Point; side: PeerSide | null };

/** Drag the paired display onto an edge; the runtime mirrors the other end. */
export default function LayoutPage({ peerName, connected = false, status }: { peerName?: string; connected?: boolean; status?: Status | null }) {
  const { t } = useT();
  const [layout, setLayout] = useState<Layout | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [pollError, setPollError] = useState("");
  const [width, setWidth] = useState(360);
  const [drag, setDrag] = useState<Drag | null>(null);
  const canvas = useRef<HTMLDivElement>(null);
  const dragSession = useRef<{ pointerId: number; offset: Point; center: Point } | null>(null);
  const pendingRef = useRef(false);
  const revision = useRef(0);
  const hintId = useId();
  const geometry = screenGeometry(width);
  const peer = peerName ?? t("workspace_peer");

  useLayoutEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const measure = () => {
      const next = element.getBoundingClientRect().width;
      if (next > 0) setWidth(next);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => { dragSession.current = null; setDrag(null); }, [width]);
  useEffect(() => {
    let active = true;
    const tick = async () => {
      if (pendingRef.current || dragSession.current || document.hidden) return;
      const requestedAt = revision.current;
      try {
        const value = await invoke<Layout>("get_layout");
        if (active && requestedAt === revision.current && !pendingRef.current && !dragSession.current) {
          setLayout(value); setPollError("");
        }
      } catch (e) { if (active) setPollError(String(e)); }
    };
    tick();
    const timer = setInterval(tick, 2000);
    return () => { active = false; clearInterval(timer); };
  }, []);

  async function setSide(peer_side: PeerSide) {
    if (!layout || pendingRef.current || peer_side === layout.peer_side) return;
    const previous = layout;
    revision.current++;
    pendingRef.current = true;
    setPending(true); setError("");
    setLayout({ peer_side });
    try { await invoke("set_layout", { cfg: { peer_side } }); }
    catch (e) { setLayout(previous); setError(String(e)); }
    finally { pendingRef.current = false; setPending(false); }
  }

  function cancelDrag() { dragSession.current = null; setDrag(null); }
  function startDrag(event: PointerEvent<HTMLButtonElement>) {
    if (event.button !== 0 || !layout || pendingRef.current || dragSession.current) return;
    const rect = canvas.current?.getBoundingClientRect();
    if (!rect) return;
    event.preventDefault();
    event.currentTarget.focus({ preventScroll: true });
    event.currentTarget.setPointerCapture(event.pointerId);
    const center = screenPosition(layout.peer_side, geometry);
    dragSession.current = { pointerId: event.pointerId, center, offset: { x: event.clientX - rect.left - center.x, y: event.clientY - rect.top - center.y } };
    setDrag({ center, side: layout.peer_side }); setError("");
  }
  function moveDrag(event: PointerEvent<HTMLButtonElement>) {
    const session = dragSession.current;
    const rect = canvas.current?.getBoundingClientRect();
    if (!session || session.pointerId !== event.pointerId || !rect) return;
    const center = { x: event.clientX - rect.left - session.offset.x, y: event.clientY - rect.top - session.offset.y };
    session.center = center;
    setDrag({ center, side: screenDropSide(center, geometry) });
  }
  function finishDrag(event: PointerEvent<HTMLButtonElement>) {
    const session = dragSession.current;
    if (!session || session.pointerId !== event.pointerId) return;
    const side = screenDropSide(session.center, geometry);
    cancelDrag();
    event.currentTarget.releasePointerCapture(event.pointerId);
    if (side) void setSide(side);
  }

  const side = layout?.peer_side ?? "right";
  const center = drag?.center ?? screenPosition(side, geometry);
  const localLocked = status?.input_locked ?? false;
  const peerLocked = connected && (status?.peer_lock_visual?.locked ?? status?.remote_input_locked ?? false);
  const effect = status?.lock_effect;
  const lockStyle = { "--lock-color": `rgb(${(effect?.color ?? [40, 210, 190]).join(", ")})` } as CSSProperties;
  const peerEffect = status?.peer_lock_visual?.effect ?? effect;
  const peerLockStyle = { "--lock-color": `rgb(${(peerEffect?.color ?? [40, 210, 190]).join(", ")})` } as CSSProperties;
  const peerLockClass = peerEffect?.rainbow ? " display-lock-rainbow" : "";
  const lockClass = effect?.rainbow ? " display-lock-rainbow" : "";
  const lockMark = <span className="display-lock-mark" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><rect x="5" y="10" width="14" height="11" rx="2" /><path d="M8 10V6a4 4 0 0 1 8 0v4" /></svg></span>;
  const tileStyle = (point: Point) => ({ left: point.x, top: point.y, width: geometry.tileWidth, height: geometry.tileHeight });
  return <div>
    <p id={hintId} className="text-xs text-ds-text-muted mb-4 leading-relaxed">{t("layout_drag_hint")}</p>
    <div ref={canvas} className="layout-canvas" role="group" aria-label={t("settings_screens")} style={{ height: geometry.height }}>
      {layout ? <>
        {drag && screenSides.map(target => <div key={target} aria-hidden="true" className={"display-drop-slot " + (drag.side === target ? "is-target" : "")} data-drop-side={target} style={tileStyle(screenPosition(target, geometry))}>
          <span>{t(`layout_${target}`)}</span>
        </div>)}
        <div className={"display-tile display-local" + lockClass} data-locked={localLocked} style={{ ...tileStyle(geometry.local), ...lockStyle }}>
          {localLocked && <>{lockMark}<span className="sr-only">{t("workspace_screen_locked")}</span></>}
          <span className="display-number" aria-hidden="true">1</span><span className="display-name">{t("workspace_local")}</span>
        </div>
        <button type="button" className={"display-tile display-peer " + (drag ? "is-dragging" : "") + peerLockClass} data-locked={peerLocked} style={{ ...tileStyle(center), ...peerLockStyle }}
          disabled={pending} data-peer-side={side} aria-label={`${t("layout_drag_screen")}: ${peer}`} aria-describedby={hintId + (peerLocked ? ` ${hintId}-lock` : "")} aria-keyshortcuts="ArrowUp ArrowDown ArrowLeft ArrowRight" title={peer}
          onPointerDown={startDrag} onPointerMove={moveDrag} onPointerUp={finishDrag} onPointerCancel={cancelDrag} onLostPointerCapture={cancelDrag}
          onKeyDown={event => {
            if (event.key === "Escape") { event.preventDefault(); cancelDrag(); return; }
            const target: Record<string, PeerSide> = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "top", ArrowDown: "bottom" };
            if (target[event.key]) { event.preventDefault(); if (!event.repeat && !dragSession.current) void setSide(target[event.key]); }
          }}>
          {peerLocked && <>{lockMark}<span id={`${hintId}-lock`} className="sr-only">{t("workspace_screen_locked")}</span></>}
          <span className="display-number" aria-hidden="true">2</span><span className="display-name">{peer}</span>
          <span className="sr-only">{t(connected ? "workspace_ready" : "workspace_waiting_short")}</span>
        </button>
      </> : <p role="status" className="absolute inset-0 flex items-center justify-center text-sm text-ds-text-muted">{t("layout_loading")}</p>}
    </div>
    <div className="flex flex-wrap items-center justify-between gap-2 mt-3 text-xs text-ds-text-muted">
      <span>{t("layout_keyboard_hint")}</span>
      <span role="status" aria-live="polite">{pending ? t("layout_saving") : layout ? `${t("layout_position_label")} ${t(`layout_${side}`)}` : ""}</span>
    </div>
    {(error || pollError) && <p role="alert" className="text-sm text-ds-danger mt-3">{error || pollError}</p>}
  </div>;
}
