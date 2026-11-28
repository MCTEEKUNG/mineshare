import AudioPage from "./Audio";
import DevicesPage from "./Devices";
import { useT } from "../i18n";
import ConnectedDevices from "../components/ConnectedDevices";

export default function AudioDevicesPage() {
  const { t } = useT();
  return (
    <div className="settings-stack gap-6">
      <ConnectedDevices audioOnly />
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
