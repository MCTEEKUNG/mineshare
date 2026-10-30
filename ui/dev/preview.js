// Browser-only design fixture. Injected only by Vite's design-preview dev mode.
// Native Tauri sessions keep their real IPC bridge.
(() => {
  if (window.__TAURI_INTERNALS__) return;
  document.title = 'MineShare · Design preview';
  let paused = false;
  const waiting = new URLSearchParams(location.search).get('connection') === 'waiting';
  const selectedPeer = { device_id: 'fda74f50-preview', display_name: 'Tee', os: waiting ? null : 'linux' };
  const demoLock = new URLSearchParams(location.search).get('lock');
  let localLocked = demoLock === 'local' || demoLock === 'both';
  const peerLocked = demoLock === 'peer' || demoLock === 'both';
  let layout = { peer_side: 'right' };
  const audio = { send_sysout: false, play_sysout: false, send_mic: false, play_mic: false, send_sysout_state: 'disabled', play_sysout_state: 'disabled', send_mic_state: 'disabled', play_mic_state: 'disabled', virtual_mic: 'vbcable', os: 'windows' };
  const devices = { outputs: [{ name: 'Headphones', is_default: true }], inputs: [{ name: 'Microphone', is_default: true }], selected_output: null, selected_input: null, selected_sysout_capture: null };
  const settings = { lock_effect: { rainbow: false, color: [255, 180, 50], animation: 'pulse', duration_ms: 1400, thickness: 8 }, mouse_sensitivity: 1, touchpad_scroll_speed: 1, mouse_rate_hz: 250, invert_scroll_y: false, invert_scroll_x: false, auto_focus_on_take_control: false };
  let nextCallback = 0;
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    transformCallback: () => ++nextCallback, unregisterCallback: () => {},
    invoke: async (command, args = {}) => {
      if (command.startsWith('plugin:event|')) return ++nextCallback;
      switch (command) {
        case 'get_status': return { peer_connected: !paused && !waiting, peer_addr: '192.168.1.108', peer_name: waiting ? null : 'Tee', local_in_remote: false, peer_in_remote: false, input_locked: localLocked, remote_input_locked: peerLocked, lock_effect: { ...settings.lock_effect }, peer_lock_visual: { locked: peerLocked, effect: { ...settings.lock_effect, color: [40, 210, 190] } }, game_drive: 'off', anticheat_warning: null, keyboard_target: 'smart', sent_pkts: 100, recv_pkts: 100, injected: 100, audio_recv: 0, inject_errs: 0, decrypt_errs: 0, keys_forwarded: 100, keys_injected: 100, local_version: '0.0.16 · Design preview', peer_version: '0.0.16 · Design preview' };
        case 'get_connection_control': return { paused, pairing_enabled: false, selected_peer: selectedPeer };
        case 'set_connections_paused': paused = args.paused; return null;
        case 'get_layout': return { ...layout };
        case 'set_layout': layout = { ...args.cfg }; return null;
        case 'get_pairing_phase': return { kind: 'none' };
        case 'get_linux_setup_status': return { linux: false };
        case 'get_transfers': return [];
        case 'list_nearby_peers': return [{ device_id: waiting ? '6289ef5a-preview' : selectedPeer.device_id, display_name: 'Tee', os: waiting ? 'windows' : 'linux', addresses: ['192.168.1.108'], trusted: true, connected: !paused && !waiting, request_supported: true }];
        case 'get_latency': return { samples: 20, last_ms: 1.2, p50_ms: 1.2, p95_ms: 2, min_ms: 1, avg_ms: 1.2, max_ms: 2, histogram: [], bin_edges_ms: [] };
        case 'get_startup_status': return { enabled: true, managed: false };
        case 'get_settings': return { ...settings };
        case 'patch_settings': Object.assign(settings, args.patch); return { ...settings };
        case 'set_input_lock': localLocked = args.locked; return null;
        case 'preview_lock_effect': return null;
        case 'get_audio_status': return { ...audio };
        case 'set_audio_toggle': audio[`${args.direction}_${args.stream}`] = args.enabled; audio[`${args.direction}_${args.stream}_state`] = args.enabled ? 'idle' : 'disabled'; return null;
        case 'list_audio_devices': return { ...devices };
        case 'set_audio_output_device': devices.selected_output = args.name; return null;
        case 'set_audio_input_device': devices.selected_input = args.name; return null;
        case 'set_sysout_capture_device': devices.selected_sysout_capture = args.name; return null;
        default: throw new Error('Design preview: this action needs the installed MineShare app.');
      }
    },
  };
})();
