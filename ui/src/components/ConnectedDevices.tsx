import { useConnectionControl } from "../connectionState";
import { useT } from "../i18n";
import WorkspaceIcon from "./WorkspaceIcon";

export default function ConnectedDevices({ audioOnly = false }: { audioOnly?: boolean }) {
  const { t } = useT();
  const { peers, control, busy, error, change } = useConnectionControl();
  const enabled = peers.length > 0 && !control?.paused;
  return <section className="panel p-5" aria-label={t(audioOnly ? "peers_audio_title" : "peers_title")}>
    <h2 className="text-sm font-semibold">{t(audioOnly ? "peers_audio_title" : "peers_title")}</h2>
    <p className="mt-2 text-xs leading-relaxed text-ds-text-muted">{t(audioOnly ? "peers_audio_hint" : "peers_input_hint")}</p>
    {enabled ? <div className="mt-4 space-y-3">
      {peers.map(peer => <article key={peer.device_id} className="rounded-2xl bg-ds-elevated p-4" aria-label={peer.display_name}>
        <div className="flex items-center gap-3">
          <WorkspaceIcon kind={peer.os.toLowerCase().includes("android") ? "phone" : "monitor"} className="size-6 shrink-0 text-ds-accent" />
          <div className="min-w-0 flex-1"><h3 className="text-sm font-medium truncate">{peer.display_name}</h3><p className="text-[11px] text-ds-text-muted mt-1">{peer.os} · {peer.device_id.slice(0, 8)}</p></div>
        </div>
        {!audioOnly && <button className={"mt-3 w-full " + (peer.input_active ? "button-primary" : "button-secondary")}
          disabled={busy || peer.input_active} aria-pressed={peer.input_active}
          onClick={() => change("select_input_peer", { deviceId: peer.device_id })}>
          {t(peer.input_active ? "peers_active" : "peers_control")}
        </button>}
        <div className="mt-3 grid gap-2 sm:grid-cols-2">
          <label className="flex gap-2 items-start text-xs cursor-pointer leading-relaxed">
            <input type="checkbox" className="mt-0.5 accent-ds-accent" checked={peer.send_audio} disabled={busy}
              onChange={event => change("set_peer_audio_route", { deviceId: peer.device_id, sendSystem: event.target.checked, receiveSystem: peer.receive_audio })} />
            <span>{t("peers_send_audio")}</span>
          </label>
          <label className="flex gap-2 items-start text-xs cursor-pointer leading-relaxed">
            <input type="checkbox" className="mt-0.5 accent-ds-accent" checked={peer.receive_audio} disabled={busy}
              onChange={event => change("set_peer_audio_route", { deviceId: peer.device_id, sendSystem: peer.send_audio, receiveSystem: event.target.checked })} />
            <span>{t("peers_receive_audio")}</span>
          </label>
        </div>
      </article>)}
      {!audioOnly && (() => { const local = !peers.some(peer => peer.input_active); return <article className="rounded-2xl bg-ds-elevated p-4" aria-label={t("workspace_local")}>
        <div className="flex items-center gap-3">
          <WorkspaceIcon kind="monitor" className="size-6 shrink-0 text-ds-accent" />
          <div className="min-w-0 flex-1"><h3 className="text-sm font-medium truncate">{t("workspace_local")}</h3></div>
        </div>
        <button className={"mt-3 w-full " + (local ? "button-primary" : "button-secondary")} disabled={busy || local} aria-pressed={local}
          onClick={() => change("select_input_peer", { deviceId: null })}>{t(local ? "peers_active" : "peers_local")}</button>
      </article>; })()}
      {peers.some(peer => peer.os.toLowerCase().includes("android")) && <p className="text-xs leading-relaxed text-ds-text-muted">{t("peers_android_audio_hint")}</p>}
    </div> : <p className="mt-4 text-xs text-ds-text-muted">{t("peers_audio_empty")}</p>}
    {error && <p role="alert" className="mt-3 text-sm text-ds-danger">{error}</p>}
  </section>;
}
