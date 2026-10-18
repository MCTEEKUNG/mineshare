export type SetupStatus = {
  linux: boolean; session: string; gnome_supported: boolean; wayland: boolean;
  input_active: boolean; input_configured: boolean; uinput_available: boolean; typeless: boolean;
  broker_configured: boolean; broker_active: boolean;
  extension_configured: boolean; extension_active: boolean; extensions_allowed: boolean;
  helpers_available: boolean; pending_login: boolean;
};
export type SetupOptions = { input: boolean; broker: boolean; pointer: boolean };
export function recommended(s: SetupStatus): SetupOptions {
  return { input: !s.input_configured || !s.uinput_available, broker: s.typeless && !s.broker_configured,
    pointer: s.wayland && s.gnome_supported && s.extensions_allowed && !s.extension_configured };
}
export function needsSetup(s: SetupStatus): boolean {
  return s.linux && (!s.input_active || (s.typeless && !s.broker_active) ||
    (s.wayland && !s.extension_active));
}
