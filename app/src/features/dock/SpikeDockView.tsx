// SPIKE (throwaway): the dock child webview's page. Solid green with a
// transparent left column; it calls a main-only command and a main-only
// plugin command, then reports how both went.
import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

function outcome(call: Promise<unknown>): Promise<string> {
  return call.then(
    () => 'allowed',
    e => `refused (${String(e).slice(0, 160)})`,
  );
}

export default function SpikeDockView() {
  useEffect(() => {
    document.documentElement.style.background = 'transparent';
    document.body.style.background = 'transparent';
    void (async () => {
      const mainOnly = await outcome(invoke('creation_preview_status'));
      const plugin = await outcome(invoke('plugin:path|resolve_directory', { directory: 11 }));
      await invoke('spike_dock_report', { mainOnly, plugin });
    })();
  }, []);
  return (
    <div style={{ position: 'fixed', inset: 0, display: 'flex' }}>
      <div style={{ width: 40, background: 'transparent' }} />
      <div style={{ flex: 1, background: '#00ff00', color: '#000', font: '14px sans-serif', padding: 8 }}>
        Spike dock layer
      </div>
    </div>
  );
}
