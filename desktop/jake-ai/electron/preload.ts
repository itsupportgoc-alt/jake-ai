import { contextBridge, ipcRenderer } from 'electron';
import { CHANNELS } from '../shared/channels';

/**
 * Minimal, explicit preload bridge. No Node APIs, filesystem or secrets are
 * exposed to the renderer: only allowlisted IPC channels.
 */
contextBridge.exposeInMainWorld('jake', {
  invoke: (channel: string, payload?: unknown) => {
    if (!(CHANNELS as readonly string[]).includes(channel)) {
      return Promise.resolve({ ok: false, error: `Channel not allowed: ${channel}` });
    }
    return ipcRenderer.invoke('jake:invoke', channel, payload ?? {});
  },
  onEvent: (callback: (event: Record<string, unknown>) => void) => {
    const listener = (_e: Electron.IpcRendererEvent, data: Record<string, unknown>) => callback(data);
    ipcRenderer.on('jake:event', listener);
    return () => ipcRenderer.removeListener('jake:event', listener);
  },
});
