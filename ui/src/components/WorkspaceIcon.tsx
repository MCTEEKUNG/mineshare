type Kind = "home" | "monitor" | "mouse" | "copy" | "shield" | "pause" | "link";
export default function WorkspaceIcon({ kind, className = "size-5" }: { kind: Kind; className?: string }) {
  return <svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" className={className}>
    {kind === "home" && <><path d="m3 10 9-7 9 7v10a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1z" /></>}
    {kind === "monitor" && <><rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 21h8m-4-5v5" /></>}
    {kind === "mouse" && <><rect x="6" y="2" width="12" height="20" rx="6" /><path d="M12 6v4" /></>}
    {kind === "copy" && <><rect x="8" y="8" width="12" height="13" rx="2" /><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3" /></>}
    {kind === "shield" && <><path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6z" /><path d="m8 12 3 3 5-6" /></>}
    {kind === "pause" && <><path d="M8 5v14M16 5v14" /></>}
    {kind === "link" && <><path d="m10 13 4-4m-7 6-1 1a4 4 0 0 0 6 6l4-4a4 4 0 0 0 0-6m1-3 1-1a4 4 0 0 0-6-6L8 6a4 4 0 0 0 0 6" transform="translate(0 -1)" /></>}
  </svg>;
}
