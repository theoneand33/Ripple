const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('electronAPI', {
  setIgnoreMouseEvents: (ignore, forward) => {
    ipcRenderer.invoke('set-ignore-mouse-events', ignore, forward);
  },
  setWindowInputShape: (rect) => ipcRenderer.send('set-window-input-shape', rect),
  getSystemMedia: () => ipcRenderer.invoke('get-system-media'),
  debugGetSystemMediaRaw: () => ipcRenderer.invoke('debug-get-system-media-raw'),
  getBluetoothStatus: () => ipcRenderer.invoke('get-bluetooth-status'),
  getCameraStatus: () => ipcRenderer.invoke('get-camera-status'),
  getMicrophoneStatus: () => ipcRenderer.invoke('get-microphone-status'),
  controlSystemMedia: (command) => ipcRenderer.invoke('control-system-media', command),
  openExternal: (url) => ipcRenderer.invoke('open-external', url),
  launchApp: (appName) => ipcRenderer.invoke('launch-app', appName),
  buildAppCache: () => ipcRenderer.invoke('build-app-cache'),
  searchApps: (query) => ipcRenderer.invoke('search-apps', query),
  getDisplays: () => ipcRenderer.invoke('get-displays'),
  setDisplay: (displayId) => ipcRenderer.invoke('set-display', displayId),
  setNotchMode: (enabled) => ipcRenderer.invoke('set-notch-mode', enabled),
  updateWindowPosition: (xPerc, yPx) => ipcRenderer.invoke('update-window-position', xPerc, yPx),
  setAutoLaunch: (enable) => process.platform !== 'darwin' ? ipcRenderer.invoke('set-auto-launch', enable) : Promise.resolve(),
  focusWindow: () => ipcRenderer.invoke('focus-window'),
  t3Load: (device) => ipcRenderer.invoke('t3-load', device),
  t3Projects: (device, environmentId) => ipcRenderer.invoke('t3-projects', device, environmentId),
  t3StartThread: (device, options) => ipcRenderer.invoke('t3-start-thread', device, options),
  openSettings: () => ipcRenderer.invoke('open-settings'),
  platform: process.platform
});
