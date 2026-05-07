import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Peer = {
  device_id: string;
  display_name: string;
  os: string;
  control_port: number;
  addresses: string[];
};

export default function LayoutPage() {
  const [peers, setPeers] = useState<Peer[]>([]);

  useEffect(() => {
    const tick = () => {
      invoke<Peer[]>("list_peers").then(setPeers).catch(() => {});
    };
    tick();
    const id = setInterval(tick, 2000);
    return () => clearInterval(id);
  }, []);

  return (
    <section>
      <p className="text-sm text-neutral-500 mb-6">
        Drag your displays to match physical arrangement. Snap a display from
        another PC next to one of yours to create a bridge edge.
      </p>

      <div className="rounded-lg border border-dashed border-neutral-300 dark:border-neutral-700 p-12 text-center text-neutral-400 mb-8">
        Layout canvas (M2)
      </div>

      <h3 className="text-sm font-semibold mb-3 text-neutral-600 dark:text-neutral-400 uppercase tracking-wide">
        Discovered peers
      </h3>
      {peers.length === 0 ? (
        <p className="text-sm text-neutral-400">
          No peers visible on the LAN yet…
        </p>
      ) : (
        <ul className="space-y-2">
          {peers.map((p) => (
            <li
              key={p.device_id}
              className="flex items-center justify-between rounded-md border border-neutral-200 dark:border-neutral-800 px-4 py-3"
            >
              <div>
                <p className="font-medium text-sm">{p.display_name}</p>
                <p className="text-xs text-neutral-500">
                  {p.os} · {p.addresses.join(", ")}:{p.control_port}
                </p>
              </div>
              <span className="inline-flex items-center gap-1 text-xs text-emerald-600">
                <span className="size-2 rounded-full bg-emerald-500" /> online
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
