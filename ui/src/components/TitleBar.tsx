import { getCurrentWindow } from "@tauri-apps/api/window";
import { useT } from "../i18n";

const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const run = (action: "minimize" | "toggleMaximize" | "close") => { void getCurrentWindow()[action](); };

/** The window has no system title bar: a drag strip across the top and the three window buttons live inside the app. */
export default function TitleBar() {
  const { t } = useT();
  if (!inTauri()) return null;
  const button = "window-button";
  return <div className="title-bar" data-tauri-drag-region onDoubleClick={() => run("toggleMaximize")}>
    <div className="window-buttons">
      <button type="button" className={button} aria-label={t("window_minimize")} title={t("window_minimize")} onClick={() => run("minimize")} onDoubleClick={event => event.stopPropagation()}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 8.5h10" /></svg>
      </button>
      <button type="button" className={button} aria-label={t("window_maximize")} title={t("window_maximize")} onClick={() => run("toggleMaximize")} onDoubleClick={event => event.stopPropagation()}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><rect x="3.5" y="3.5" width="9" height="9" rx="1.5" /></svg>
      </button>
      <button type="button" className={button + " window-close"} aria-label={t("window_close")} title={t("window_close")} onClick={() => run("close")} onDoubleClick={event => event.stopPropagation()}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" /></svg>
      </button>
    </div>
  </div>;
}
