import { useState, type ReactNode } from "react";

/** Mount optional controls only when opened, so hidden panels do not poll IPC. */
export default function Disclosure({ title, children }: { title: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return <details className="disclosure" onToggle={event => setOpen(event.currentTarget.open)}>
    <summary>{title}</summary>
    {open ? <div className="disclosure-content">{children}</div> : null}
  </details>;
}
