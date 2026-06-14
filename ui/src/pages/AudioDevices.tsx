import AudioPage from "./Audio";
import DevicesPage from "./Devices";
import { useT } from "../i18n";

export default function AudioDevicesPage() {
  const { t } = useT();
  return (
    <div className="flex flex-col gap-10">
      <section>
        <h3 className="text-sm font-semibold text-slate-300 mb-3">{t("section_audio")}</h3>
        <AudioPage />
      </section>
      <section>
        <h3 className="text-sm font-semibold text-slate-300 mb-3">{t("section_devices")}</h3>
        <DevicesPage />
      </section>
    </div>
  );
}
