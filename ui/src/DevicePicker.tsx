import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "./i18n";
import { selectedPeerLabel, useConnectionControl } from "./connectionState";
import WorkspaceIcon from "./components/WorkspaceIcon";

type NearbyPeer = { device_id: string; display_name: string; os: string; addresses: string[]; trusted: boolean; connected: boolean; request_supported: boolean };

export default function DevicePicker({ connected, onClose }: { connected: boolean; onClose: () => void }) {
  const { t } = useT();
  const { busy, error, control, change } = useConnectionControl();
  const [peers, setPeers] = useState<NearbyPeer[] | null>(null);
  const [selected, setSelected] = useState<NearbyPeer | null>(null);
  const [loadError, setLoadError] = useState("");
  const dialog = useRef<HTMLDivElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  const previous = useRef<HTMLElement | null>(null);
  useEffect(() => {
    previous.current = document.activeElement as HTMLElement;
    close.current?.focus();
    return () => previous.current?.focus();
  }, []);
  useEffect(() => {
    let live = true;
    const tick = async () => {
      try { const list = await invoke<NearbyPeer[]>("list_nearby_peers"); if (live) { setPeers(list); setLoadError(""); } }
      catch (e) { if (live) setLoadError(String(e)); }
    };
    tick(); const timer = setInterval(tick, 1000);
    return () => { live = false; clearInterval(timer); };
  }, []);
  const current = peers?.find(peer => peer.device_id === selected?.device_id);
  return <div className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm flex items-center justify-center p-6">
    <div ref={dialog} role="dialog" aria-modal="true" aria-labelledby="nearby-title" tabIndex={-1} className="panel w-full max-w-lg p-6 max-h-[calc(100vh-48px)] overflow-y-auto shadow-2xl" onKeyDown={event => {
      if (event.key === "Escape" && !busy) { event.stopPropagation(); onClose(); }
      if (event.key === "Tab") {
        const buttons = Array.from(dialog.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []);
        const first = buttons[0], last = buttons[buttons.length - 1];
        if (!first) { event.preventDefault(); dialog.current?.focus(); }
        else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }
    }}>
      <div className="flex items-start justify-between gap-4">
        <h2 id="nearby-title" className="text-xl font-semibold">{t(selected ? "nearby_confirm" : "nearby_title")}</h2>
        <button ref={close} disabled={busy} onClick={onClose} className="text-link p-1" aria-label={t("nearby_close")}>✕</button>
      </div>
      {selected ? <>
        <div className="my-6 rounded-2xl bg-ds-hover p-5 flex items-center gap-4"><WorkspaceIcon kind="monitor" className="size-8 shrink-0 text-ds-text-muted" /><div className="min-w-0"><p className="font-semibold break-words">{selected.display_name}</p><p className="text-xs text-ds-text-muted mt-1 break-all">{selected.os} · {selected.addresses.join(", ")}</p><p className="text-[11px] text-ds-text-muted mt-1">{selected.device_id.slice(0, 8)}</p></div></div>
        <p className="muted">{t("nearby_confirm_hint")}</p>
        {connected && <p className="muted mt-3">{t("nearby_switch_hint")}</p>}
        {!current && <p role="status" className="muted mt-3">{t("nearby_gone")}</p>}
        <div className="flex gap-3 justify-end mt-6">
          <button disabled={busy} className="button-secondary" onClick={() => setSelected(null)}>{t("nearby_back")}</button>
          <button disabled={busy || !current?.request_supported || current.connected || control?.paused} className="button-primary" onClick={async () => {
            if (await change("send_pairing_request", { deviceId: selected.device_id })) onClose();
          }}>{t("nearby_send")}</button>
        </div>
      </> : <>
        <p className="muted mt-3 mb-5">{t("nearby_hint")}</p>
        {control?.selected_peer && <div className="rounded-xl bg-ds-hover p-3 mb-4 text-xs leading-relaxed">
          <p className="break-words">{t("workspace_selected_computer")} {selectedPeerLabel(control.selected_peer)}</p>
          <p className="text-ds-text-muted mt-1">{t("nearby_selected_hint")}</p>
        </div>}
        <div className="space-y-2">
          {peers?.map(peer => <button key={peer.device_id} disabled={busy || peer.connected || !peer.request_supported || control?.paused} onClick={() => setSelected(peer)} className="w-full flex items-center gap-4 rounded-2xl border border-ds-border p-4 text-left hover:bg-ds-hover disabled:opacity-50">
            <WorkspaceIcon kind="monitor" className="size-7 shrink-0 text-ds-text-muted" />
            <span className="min-w-0 flex-1"><span className="block font-medium truncate">{peer.display_name}</span><span className="block text-xs text-ds-text-muted mt-1 break-all">{peer.os} · {peer.addresses.join(", ")}</span><span className="block text-[11px] text-ds-text-muted mt-1">{peer.device_id.slice(0, 8)}{peer.trusted ? ` · ${t("nearby_paired")}` : ""}</span></span>
            <span className="text-xs text-ds-text-muted shrink-0 max-w-24">{t(peer.connected ? "workspace_connected" : !peer.request_supported ? "nearby_update" : peer.device_id === control?.selected_peer?.device_id ? "nearby_selected" : "nearby_select")}</span>
          </button>)}
          {!peers && !loadError && <p role="status" className="muted py-6">{t("nearby_searching")}</p>}
          {peers?.length === 0 && <p role="status" className="muted py-6">{t("nearby_empty")}</p>}
        </div>
      </>}
      {(loadError || error) && <p role="alert" className="mt-4 text-sm text-ds-danger">{loadError || error}</p>}
    </div>
  </div>;
}
