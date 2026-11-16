import AudioPage from "./Audio";
import DevicesPage from "./Devices";
import { useT } from "../i18n";

export default function AudioDevicesPage() {
  const { t } = useT();
  return (
    <div className="settings-stack gap-6">
      <section>
        <h2 className="section-title">{t("section_audio")}</h2>
        <AudioPage />
      </section>
      <section>
        <DevicesPage />
      </section>
    </div>
  );
}
