"use strict";
import x11Module from "x11";
import { setT3TokenFile, t3Load, t3Projects, t3StartThread } from "./t3.mjs";
import { tokenizeArgs, parseCommand } from "./command.mjs";

const {
  app,
  BrowserWindow,
  screen,
  ipcMain,
  shell,
  Tray,
  Menu,
  nativeImage,
} = require("electron");
const path = require("node:path");
const { createRequire } = require("node:module");
const fs = require("fs");

if (process.platform === "linux") {
  app.commandLine.appendSwitch("enable-transparent-visuals");
}
const x11 = process.platform === "linux" ? x11Module : null;
let tray = null;
let mainWindow = null;
let settingsWindow = null;
let mainWindowReady = false;
let mainWindowInputShapeReady = false;
let x11Display = null;
let x11Shape = null;
let pendingInputShape = null;
let inputShapeCheckPending = false;
let activeDisplayId = null;
let notchModeEnabled = false;
let macNotchBridge;

const getMacNotchBridge = () => {
  if (process.platform !== "darwin") return null;
  if (macNotchBridge !== undefined) return macNotchBridge;

  try {
    const appRequire = createRequire(path.join(app.getAppPath(), "package.json"));
    const addonPath = app.isPackaged
      ? path.join(process.resourcesPath, `mac_notch-${process.arch}.node`)
      : path.join(app.getAppPath(), "native/mac-notch/prebuilds", `mac_notch-${process.arch}.node`);
    macNotchBridge = appRequire(addonPath);
  } catch (error) {
    macNotchBridge = null;
    console.error("macOS Notch Mode native bridge is unavailable:", error);
  }

  return macNotchBridge;
};

const moveWindowToNotch = () => {
  if (!mainWindow || !notchModeEnabled || process.platform !== "darwin") return;
  const bridge = getMacNotchBridge();
  if (bridge) {
    const topGap = bridge.setWindowTopEdge(mainWindow.getNativeWindowHandle(), true);
    if (Math.abs(topGap) > 0.5) {
      console.warn(`Notch Mode native screen-top gap is ${topGap.toFixed(1)} pt`);
    }
  }
};

const showMainWindow = () => {
  if (!mainWindow || !mainWindowReady) return;
  if (process.platform === "linux" && !mainWindowInputShapeReady) return;

  mainWindow.show();
  mainWindow.setAlwaysOnTop(
    true,
    process.platform === "linux" ? "screen-saver" : "pop-up-menu",
  );
  mainWindow.focus();
  moveWindowToNotch();
};

const applyLinuxInputShape = (rect) => {
  if (!mainWindow || process.platform !== "linux") return;

  pendingInputShape = rect;
  const { x, y, width, height } = rect;
  // Keep the visual window rectangular and transparent. Changing ShapeBounding
  // on every animation frame races the compositor and can flash a black edge;
  // only ShapeInput is needed to let clicks pass through outside the Island.
  if (!x11Display || !x11Shape) return;

  const windowId = mainWindow.getNativeWindowHandle().readUInt32LE(0);
  const scaleFactor = Number.isFinite(rect.scaleFactor) && rect.scaleFactor > 0
    ? rect.scaleFactor
    : 1;
  const inputRect = [
    Math.floor(x * scaleFactor),
    Math.floor(y * scaleFactor),
    Math.ceil(width * scaleFactor),
    Math.ceil(height * scaleFactor),
  ];

  x11Shape.Rectangles(
    x11Shape.Op.Set,
    x11Shape.Kind.Input,
    windowId,
    0,
    0,
    [inputRect],
    x11Shape.Ordering.Unsorted,
  );

  if (mainWindowInputShapeReady || inputShapeCheckPending) return;
  inputShapeCheckPending = true;
  x11Shape.GetRectangles(windowId, x11Shape.Kind.Input, (error, result) => {
    inputShapeCheckPending = false;
    if (error) {
      console.error("Failed to read Linux window input shape:", error);
      return;
    }

    const actual = result.rectangles?.[0];
    const bounds = mainWindow?.getBounds();
    const scale = rect.scaleFactor || 1;
    if (!actual || !bounds ||
        actual[2] >= bounds.width * scale || actual[3] >= bounds.height * scale) {
      console.error("Linux window input shape still covers the full window:", result.rectangles);
      return;
    }

    mainWindowInputShapeReady = true;
    showMainWindow();
  });
};

const initializeLinuxInputShape = () => {
  if (process.platform !== "linux") return;

  x11.createClient((error, display) => {
    if (error) {
      console.error("Failed to connect to X11 for input shaping:", error);
      return;
    }

    display.client.on("error", (clientError) => {
      console.error("X11 input-shape connection error:", clientError);
    });
    display.client.require("shape", (shapeError, shape) => {
      if (shapeError) {
        console.error("X11 Shape extension is unavailable:", shapeError);
        display.client.terminate();
        return;
      }

      x11Display = display;
      x11Shape = shape;
      if (pendingInputShape) applyLinuxInputShape(pendingInputShape);
    });
  });
};

const { exec, spawn } = require('child_process');

// Only our own windows may call privileged IPC handlers (launch, open-external, ...).
const trusted = (event) => [mainWindow, settingsWindow].some((w) => w && event.sender === w.webContents);

// Runs a PowerShell script via -EncodedCommand (cmd.exe cuts a multi-line -Command at the
// first newline). Resolves trimmed stdout, or "" on failure.
const runPs = (script, maxBuffer = 1024 * 1024) => new Promise((resolve) => {
  const enc = Buffer.from(script, "utf16le").toString("base64");
  exec(`powershell -NoProfile -EncodedCommand ${enc}`, { maxBuffer, encoding: "utf8", timeout: 30000 }, (err, out) => {
    if (err) console.error("PowerShell failed:", err.message);
    resolve(err ? "" : out.trim());
  });
});
const runPsList = async (script, maxBuffer) => {
  try {
    const d = JSON.parse(await runPs(script, maxBuffer));
    return Array.isArray(d) ? d : d ? [d] : [];
  } catch {
    return [];
  }
};

const MEDIA_PS = `[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Runtime.WindowsRuntime; $manager = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]::RequestAsync().GetAwaiter().GetResult(); $session = $manager.GetCurrentSession(); if ($session) { $props = $session.TryGetMediaPropertiesAsync().GetAwaiter().GetResult(); $playback = $session.GetPlaybackInfo(); $status = $playback.PlaybackStatus; $thumbnail = $props.Thumbnail; $artwork = ''; if ($thumbnail) { try { $stream = $thumbnail.OpenReadAsync().GetAwaiter().GetResult(); $buffer = New-Object byte[] $stream.Size; $reader = New-Object Windows.Storage.Streams.DataReader $stream; $reader.LoadAsync($stream.Size).GetAwaiter().GetResult() | Out-Null; $reader.ReadBytes($buffer); $artwork = 'data:image/png;base64,' + [Convert]::ToBase64String($buffer); $reader.Close(); $stream.Close(); } catch { } } $info = @{ Title = $props.Title; Artist = $props.Artist; Album = $props.AlbumTitle; Status = $status.ToString().ToLower(); Source = $session.SourceAppUserModelId; Artwork = $artwork }; return $info | ConvertTo-Json -Compress; } return 'null';`;

// --- App discovery providers ---

// Scans Start Menu .lnk files and resolves them to Win32 exe paths.
// Skips shortcuts targeting explorer.exe (UWP launchers) or WindowsApps.
function discoverStartMenu() {
  return new Promise((resolve) => {
    const script = `
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$shell   = New-Object -ComObject WScript.Shell
$dirs    = @("$env:ProgramData\\Microsoft\\Windows\\Start Menu\\Programs","$env:APPDATA\\Microsoft\\Windows\\Start Menu\\Programs")
$results = [System.Collections.Generic.List[object]]::new()
foreach ($dir in $dirs) {
  if (-not (Test-Path $dir)) { continue }
  Get-ChildItem $dir -Recurse -Filter '*.lnk' -EA SilentlyContinue | ForEach-Object {
    try {
      $target = $shell.CreateShortcut($_.FullName).TargetPath
      if ($target -and $target.EndsWith('.exe') -and
          $target -notlike '*\\\\explorer.exe' -and
          $target -notmatch 'WindowsApps' -and
          (Test-Path $target -EA SilentlyContinue)) {
        $results.Add([PSCustomObject]@{ name = $_.BaseName; type = 'win32'; path = $target })
      }
    } catch {}
  }
}
@($results) | ConvertTo-Json -Compress -Depth 2
`;
    resolve(runPsList(script, 5 * 1024 * 1024));
  });
}

// Gets UWP / Store apps via Get-StartApps.
// UWP entries have AppID in the form PackageFamilyName!AppId (contains '!').
function discoverUWP() {
  return new Promise((resolve) => {
    const script = `
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$results = [System.Collections.Generic.List[object]]::new()
Get-StartApps -EA SilentlyContinue | ForEach-Object {
  if ($_.AppID -match '.+!.+') {
    $results.Add([PSCustomObject]@{ name = $_.Name; type = 'uwp'; appId = $_.AppID })
  }
}
@($results) | ConvertTo-Json -Compress -Depth 2
`;
    resolve(runPsList(script, 2 * 1024 * 1024));
  });
}

// Converts provider-specific shapes to { name, launch } and deduplicates.
// win32: launch = exe path   |   uwp: launch = shell:AppsFolder\\appId
async function buildCache() {
  const [startMenu, uwp] = await Promise.all([
    discoverStartMenu(),
    discoverUWP(),
  ]);
  const seen = new Set();
  const entries = [];
  for (const item of [...startMenu, ...uwp]) {
    if (!item.name || !(item.path || item.appId)) continue;
    const launch =
      item.type === "uwp" ? `shell:AppsFolder\\${item.appId}` : item.path;
    const key = launch.toLowerCase();
    if (!seen.has(key)) {
      seen.add(key);
      entries.push({ name: item.name, launch });
    }
  }
  return entries.sort((a, b) => a.name.localeCompare(b.name));
}

// --- Launch abstraction ---
function launchWindows(input) {
  const trimmed = input.trim();

  // UWP apps and schemes
  if (trimmed.startsWith("shell:")) {
    const safe = trimmed.replace(/'/g, "''");
    exec(
      `powershell -NoProfile -WindowStyle Hidden -Command "Start-Process '${safe}'"`,
    );
    return;
  }

  // Paths with slashes (ex: C:\Program Files\App.exe)
  if (/[\\\/]/.test(trimmed)) {
    const { exe, args } = parseCommand(trimmed);

    // If no arguments, use native OS approach for best compatibility
    if (args.length === 0) {
      if (exe.toLowerCase().endsWith(".url")) {
        try {
          const content = fs.readFileSync(exe, "utf8");
          const m = content.match(/^URL=(.+)$/im);
          if (m) shell.openExternal(m[1].trim());
        } catch {}
        return;
      }
      shell.openPath(exe).then((err) => {
        if (err) exec(`start "" "${exe}"`);
      });
      return;
    }

    // Arguments provided - spawn exactly to prevent execution escaping vulnerabilities
    const finalExe = /[\\/]/.test(exe) && !/\.[^\\.]+$/.test(exe) ? exe + ".exe" : exe;

    // cmd / bat scripts must run via cmd.exe
    if (/\.(cmd|bat)$/i.test(finalExe)) {
      const child = spawn("cmd.exe", ["/c", finalExe, ...args], {
        shell: false,
        detached: true,
        stdio: "ignore",
      });
      child.on("error", () => {});
      child.unref();
      return;
    }

    // powershell scripts
    if (/\.ps1$/i.test(finalExe)) {
      const child = spawn(
        "powershell.exe",
        ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", finalExe, ...args],
        { shell: false, detached: true, stdio: "ignore" },
      );
      child.on("error", () => {});
      child.unref();
      return;
    }

    const child = spawn(finalExe, args, {
      shell: false,
      detached: true,
      stdio: "ignore",
    });
    child.on("error", () => {}); 
    child.unref();
    return;
  }

  // App Paths or raw executables
  if (trimmed.includes(" ")) {
    const safe = trimmed.replace(/'/g, "''");
    exec(
      `powershell -NoProfile -WindowStyle Hidden -Command "Start-Process '${safe}'"`,
    );
  } else {
    exec(`start "" ${trimmed}`);
  }
}

ipcMain.handle("set-ignore-mouse-events", (event, ignore, forward) => {
  if (mainWindow && event.sender === mainWindow.webContents) {
    // Linux cannot forward mouse movement while events are ignored. Keep the
    // window interactive there and use its native shape for click-through.
    if (process.platform === "linux") return;
    mainWindow.setIgnoreMouseEvents(ignore, { forward: forward || false });
  }
});

ipcMain.on("set-window-input-shape", (event, rect) => {
  if (process.platform !== "linux" || !mainWindow || event.sender !== mainWindow.webContents) return;

  const { x, y, width, height } = rect || {};
  if (![x, y, width, height].every(Number.isFinite) || width <= 0 || height <= 0) {
    return;
  }

  try {
    applyLinuxInputShape(rect);
  } catch (error) {
    console.error("Failed to set Linux window input shape:", error);
  }
});

ipcMain.handle("focus-window", (event) => {
  if (mainWindow && event.sender === mainWindow.webContents) {
    mainWindow.focus();
  }
});

ipcMain.handle("open-external", async (event, url) => {
  // file:// is allowed for documents but not for anything that would execute.
  if (!trusted(event) || !/^(https?|file):\/\//i.test(String(url)) || /^file:.*\.(exe|bat|cmd|com|ps1|msi|scr|lnk|sh|app|desktop|jar)$/i.test(String(url))) return;
  await shell.openExternal(url);
});

ipcMain.handle("t3-load", (event, device) => trusted(event) && t3Load(device));
ipcMain.handle("t3-projects", (event, device, environmentId) => trusted(event) && t3Projects(device, environmentId));
ipcMain.handle("t3-start-thread", (event, device, options) => trusted(event) && t3StartThread(device, options));

ipcMain.handle("launch-app", async (event, appName) => {
  if (!trusted(event) || typeof appName !== "string") return;
  const platform = process.platform;
  if (platform === "win32") return launchWindows(appName);
  // No shell: the text is split into argv, so it can't chain commands.
  const [cmd, ...args] = platform === "darwin" ? ["open", "-a", appName] : tokenizeArgs(appName);
  if (!cmd) return;
  const child = spawn(cmd, args, { detached: true, stdio: "ignore" });
  child.on("error", () => {});
  child.unref();
});

ipcMain.handle("build-app-cache", async () => {
  if (process.platform !== "win32") return;
  const cacheFile = path.join(app.getPath("userData"), "app-cache.json");
  try {
    const entries = await buildCache();
    fs.writeFileSync(cacheFile, JSON.stringify(entries));
  } catch {
    // Silently ignore cache build failures
  }
});

ipcMain.handle("search-apps", async (event, query) => {
  if (process.platform !== "win32" || !query) return [];
  const cacheFile = path.join(app.getPath("userData"), "app-cache.json");
  try {
    if (!fs.existsSync(cacheFile)) return [];
    const data = JSON.parse(fs.readFileSync(cacheFile, "utf8"));
    const q = query.toLowerCase();
    return data
      .filter((a) => a.name && a.name.toLowerCase().includes(q))
      .slice(0, 8);
  } catch {
    return [];
  }
});

ipcMain.handle("get-displays", () => {
  const displays = screen.getAllDisplays();
  return displays.map((d) => ({
    id: d.id,
    label: d.label || `Display ${d.id}`,
    bounds: d.bounds,
  }));
});

ipcMain.handle("set-display", (event, displayId) => {
  if (mainWindow) {
    const displays = screen.getAllDisplays();
    const targetDisplay =
      displays.find((d) => d.id.toString() === displayId.toString()) ||
      screen.getPrimaryDisplay();

    activeDisplayId = targetDisplay.id;
    const { x, y, width, height } = targetDisplay.bounds;
    const isLinux = process.platform === "linux";

    mainWindow.setBounds({ x, y, width, height });
    if (!isLinux) {
      // Avoid setFullScreen to prevent covering taskbars and causing focus issues
      // mainWindow.setFullScreen(true);
    }

    showMainWindow();
  }
});

ipcMain.handle("set-notch-mode", (event, enabled) => {
  if (process.platform !== "darwin") return;
  notchModeEnabled = Boolean(enabled);

  if (!mainWindow) return;
  if (notchModeEnabled) {
    if (!mainWindowReady) mainWindow.show();
    mainWindow.setAlwaysOnTop(true, "pop-up-menu");
    mainWindow.focus();
    moveWindowToNotch();
    return;
  }

  const bridge = getMacNotchBridge();
  if (bridge) bridge.setWindowTopEdge(mainWindow.getNativeWindowHandle(), false);
  mainWindow.setAlwaysOnTop(true, "pop-up-menu");

  const targetDisplay = screen.getAllDisplays().find(
    (display) => display.id === activeDisplayId,
  ) || screen.getPrimaryDisplay();
  mainWindow.setBounds(targetDisplay.bounds);
  showMainWindow();
});

ipcMain.handle("set-auto-launch", (event, enable) => {
  if (!trusted(event)) return;
  if (process.platform === "linux") {
    const autostartPath = path.join(
      app.getPath("home"),
      ".config",
      "autostart",
    );
    const desktopFilePath = path.join(autostartPath, "ripple.desktop");

    try {
      if (enable) {
        if (!fs.existsSync(autostartPath)) {
          fs.mkdirSync(autostartPath, { recursive: true });
        }
        const desktopFileContent = `[Desktop Entry]
Type=Application
Version=1.0
Name=Ripple
Comment=Ripple Desktop Assistant
Exec="${app.getPath("exe")}"\nIcon=${getIconPath()}
Terminal=false
`;
        fs.writeFileSync(desktopFilePath, desktopFileContent);
      } else {
        if (fs.existsSync(desktopFilePath)) {
          fs.unlinkSync(desktopFilePath);
        }
      }
    } catch (e) {
      console.error("Failed to set auto-launch on Linux:", e);
    }
  } else if (process.platform === "win32") {
    try {
      app.setLoginItemSettings({
        openAtLogin: enable,
        path: app.getPath("exe"),
      });
    } catch (e) {
      console.error("Failed to set login item settings on Windows:", e);
    }
  }
});

const getIconPath = () => {
  const ext = "png";
  if (app.isPackaged) {
    const resPath = path.join(process.resourcesPath, `icon.${ext}`);
    const assetsPath = path.join(
      process.resourcesPath,
      `assets/icons/icon.${ext}`,
    );

    if (fs.existsSync(resPath)) return resPath;
    if (fs.existsSync(assetsPath)) return assetsPath;

    return resPath;
  }
  return path.join(__dirname, `../../src/assets/icons/icon.${ext}`);
};

const loadRenderer = (win, hash) => {
  if (!app.isPackaged || process.env.NODE_ENV === "development") {
    win.loadURL(`http://localhost:5173${hash ? `/#${hash}` : ""}`);
  } else {
    win.loadFile(path.join(__dirname, "../renderer/main_window/index.html"), hash ? { hash } : undefined);
  }
};

const openSettingsWindow = () => {
  if (settingsWindow) {
    settingsWindow.show();
    settingsWindow.focus();
    return;
  }
  settingsWindow = new BrowserWindow({
    width: 520,
    height: 720,
    minWidth: 420,
    minHeight: 400,
    title: "Ripple Settings",
    backgroundColor: "#000000",
    autoHideMenuBar: true,
    icon: getIconPath(),
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      devTools: false,
    },
  });
  lockNavigation(settingsWindow);
  settingsWindow.on("closed", () => {
    settingsWindow = null;
  });
  loadRenderer(settingsWindow, "settings");
};

ipcMain.handle("open-settings", () => openSettingsWindow());

// A link inside the page (e.g. in an AI answer) must never load in a window that has the preload API.
const lockNavigation = (win) => {
  const block = (e, url) => {
    if (url !== win.webContents.getURL()) e.preventDefault();
  };
  win.webContents.on("will-navigate", block);
  win.webContents.on("will-redirect", block);
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:\/\//i.test(url)) shell.openExternal(url);
    return { action: "deny" };
  });
};

const createWindow = () => {
  mainWindowReady = false;
  mainWindowInputShapeReady = false;
  const primaryDisplay = screen.getPrimaryDisplay();
  activeDisplayId = primaryDisplay.id;
  const { x, y, width, height } = primaryDisplay.bounds;
  const isLinux = process.platform === "linux";
  const isWindows = process.platform === "win32";
  const isMac = process.platform === "darwin";

  const winWidth = width;
  const winHeight = height;
  const winX = x;
  const winY = y;

  const windowType = isWindows ? "toolbar" : "panel";

  mainWindow = new BrowserWindow({
    width: winWidth,
    height: winHeight,
    x: winX,
    y: winY,
    backgroundColor: "#00000000",
    transparent: true,
    alwaysOnTop: true,
    resizable: false,
    frame: false,
    ...(isWindows ? {} : { thickFrame: false }),
    hasShadow: false,
    skipTaskbar: true,
    icon: getIconPath(),
    ...(isMac ? { hiddenInMissionControl: true } : {}),
    ...(windowType ? { type: windowType } : {}),
    fullscreen: false,
    visibleOnFullScreen: true,
    acceptFirstMouse: true,
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      devTools: false,
    },
    show: !isLinux,
  });

  if (!isLinux) {
    mainWindow.setIgnoreMouseEvents(true, { forward: true });
  } else {
    mainWindow.setIgnoreMouseEvents(false);
  }

  const showDelay = isLinux ? 500 : 0;

  mainWindow.once("ready-to-show", () => {
    setTimeout(() => {
      mainWindowReady = true;
      showMainWindow();
    }, showDelay);
  });

  setTimeout(() => {
    if (mainWindow && !mainWindow.isVisible()) {
      mainWindowReady = true;
      showMainWindow();
    }
  }, 5000);

  mainWindow.on("close", () => {
    if (process.platform === "darwin" && notchModeEnabled) {
      const bridge = getMacNotchBridge();
      if (bridge) bridge.setWindowTopEdge(mainWindow.getNativeWindowHandle(), false);
    }
  });

  lockNavigation(mainWindow);
  mainWindow.on("closed", () => {
    mainWindow = null;
  });

  try {
    mainWindow.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
  } catch (_) {}

  loadRenderer(mainWindow);
};

app.whenReady().then(() => {
  setT3TokenFile(path.join(app.getPath("userData"), "t3-mcp-tokens.json"));
  if (process.platform === "darwin") {
    app.dock.hide();
  }
  initializeLinuxInputShape();
  createWindow();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow();
    }
  });

  try {
    const iconPath = getIconPath();
    const icon = nativeImage.createFromPath(iconPath);
    const trayIcon = icon.resize({ width: 16, height: 16 });
    tray = new Tray(trayIcon);
    const contextMenu = Menu.buildFromTemplate([
      {
        label: "Show/Hide Ripple",
        click: () => {
          if (mainWindow) {
            if (mainWindow.isVisible()) {
              mainWindow.hide();
            } else {
              showMainWindow();
            }
          }
        },
      },
      { label: "Settings", click: openSettingsWindow },
      { type: "separator" },
      {
        label: "Quit",
        click: () => {
          app.quit();
        },
      },
    ]);
    tray.setToolTip("Ripple");
    tray.setContextMenu(contextMenu);
  } catch (e) {
    console.error("Failed to create tray:", e);
  }
});

app.on("before-quit", () => {
  if (x11Display?.client) x11Display.client.terminate();
});

ipcMain.handle("get-system-media", async () => {
  return new Promise((resolve) => {
    const platform = process.platform;

    if (platform === "darwin") {
      const script = `
            tell application "System Events"
                set spotifyRunning to (name of every process) contains "Spotify"
                set musicRunning to (name of every process) contains "Music"
            end tell
            if spotifyRunning then
                try
                    tell application "Spotify"
                        set mediaState to player state as string
                        set songName to name of current track
                        set artistName to artist of current track
                        set albumName to album of current track
                        try
                            set artUrl to artwork url of current track
                        on error
                            set artUrl to ""
                        end try
                    end tell
                    return "Spotify" & "||" & mediaState & "||" & songName & "||" & artistName & "||" & albumName & "||" & artUrl
                on error
                    return "Error"
                end try
            else if musicRunning then
                try
                    tell application "Music" 
                        set mediaState to player state as string
                        set songName to name of current track
                        set artistName to artist of current track
                        set albumName to album of current track
                    end tell
                    return "Music" & "||" & mediaState & "||" & songName & "||" & artistName & "||" & albumName & "||" & "" 
                on error
                    return "Error"
                end try
            else
                return "None"
            end if
            `;
      exec(`osascript -e '${script}'`, (error, stdout) => {
        if (error) {
          return resolve(null);
        }
        const output = stdout.trim();

        if (!output || output === "None" || output === "Error")
          return resolve(null);

        const parts = output.split("||");
        if (parts.length >= 4) {
          resolve({
            name: parts[2],
            artist: parts[3],
            album: parts[4],
            artwork_url: parts[5] || null,
            state: parts[1] === "playing" ? "playing" : "paused",
            source: parts[0],
          });
        } else {
          resolve(null);
        }
      });
    } else if (platform === "win32") {
      runPs(MEDIA_PS, 10 * 1024 * 1024).then((stdout) => {
        if (!stdout || stdout === "null" || stdout === "'null'") {
          // Fallback: try reading Spotify window title
          exec(
            `powershell -NoProfile -Command "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Get-Process | Where-Object {$_.ProcessName -eq 'Spotify'} | Select-Object MainWindowTitle"`,
            { encoding: "utf8" },
            (err, out) => {
              if (err || !out) return resolve(null);
              const title = out
                .split("\n")
                .find((l) => l.includes("-"))
                ?.trim();
              if (!title) return resolve(null);
              const [artist, ...songParts] = title.split(" - ");
              resolve({
                name: songParts.join(" - ") || title,
                artist: artist || "Unknown",
                state: "playing",
                source: "Spotify",
              });
            },
          );
          return;
        }
        try {
          const data = JSON.parse(stdout);
          resolve({
            name: data.Title || "Unknown Title",
            artist: data.Artist || "Unknown Artist",
            album: data.Album || "",
            artwork_url: data.Artwork || null,
            state: data.Status === "playing" ? "playing" : "paused",
            source: data.Source || "System",
          });
        } catch (e) {
          console.error("get-system-media: failed to parse PowerShell JSON:", e);
          resolve(null);
        }
      });
    } else if (platform === "linux") {
      exec(
        'playerctl metadata --format "{{title}}||{{artist}}||{{album}}||{{status}}"',
        (err, stdout) => {
          if (err || !stdout) return resolve(null);
          const parts = stdout.trim().split("||");
          resolve({
            name: parts[0],
            artist: parts[1],
            album: parts[2],
            state: parts[3].toLowerCase(),
            source: "System",
          });
        },
      );
    } else {
      resolve(null);
    }
  });
});

ipcMain.handle("get-bluetooth-status", async () => {
  return new Promise((resolve) => {
    const platform = process.platform;
    if (platform === "darwin") {
      exec("system_profiler SPBluetoothDataType -json", (error, stdout) => {
        if (error) return resolve(false);
        try {
          const data = JSON.parse(stdout);
          const bluetoothData = data.SPBluetoothDataType[0];
          const hasConnectedDevices =
            bluetoothData.device_connected &&
            bluetoothData.device_connected.length > 0;
          resolve(hasConnectedDevices);
        } catch (e) {
          resolve(false);
        }
      });
    } else if (platform === "win32") {
      const psScript = `@(Get-PnpDevice -Class Bluetooth -ErrorAction SilentlyContinue | Where-Object { $_.Status -eq 'OK' -and $_.Present -eq $true -and $_.InstanceId -match 'BTHENUM' }).Count -gt 0`;
      exec(`powershell -NoProfile -Command "${psScript}"`, (error, stdout) => {
        if (error) return resolve(false);
        resolve(stdout.trim().toLowerCase() === "true");
      });
    } else if (platform === "linux") {
      exec("bluetoothctl devices Connected", (error, stdout) => {
        if (error) return resolve(false);
        resolve(stdout.trim().length > 0);
      });
    } else {
      resolve(false);
    }
  });
});

ipcMain.handle("get-camera-status", async () => {
  return new Promise((resolve) => {
    const platform = process.platform;
    if (platform === "darwin") {
      exec('ioreg -l | grep -E "FrontCameraActive|FrontCameraStreaming"', (error, stdout) => {
        resolve(stdout ? stdout.includes('= Yes') : false);
      });
    } else if (platform === "win32") {
      const psScript = `
        $inUse = $false
        $keys = Get-ChildItem -Path "HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\webcam" -Recurse -ErrorAction SilentlyContinue
        foreach ($key in $keys) {
            $val = Get-ItemProperty -Path $key.PSPath -Name "LastUsedTimeStop" -ErrorAction SilentlyContinue
            if ($val -and $val.LastUsedTimeStop -eq 0) {
                $inUse = $true
                break
            }
        }
        $inUse
      `;
      runPs(psScript).then((out) => resolve(out.toLowerCase() === "true"));
    } else if (platform === "linux") {
      exec("fuser /dev/video* 2>/dev/null", (error, stdout) => {
        resolve(stdout.trim().length > 0);
      });
    } else {
      resolve(false);
    }
  });
});

ipcMain.handle("get-microphone-status", async () => {
  return new Promise((resolve) => {
    const platform = process.platform;
    if (platform === "darwin") {
      exec('ioreg -l | grep -E "IOAudioStreamActive|IOAudioEngine|IOAudioStream" | grep -i "Yes"', (error, stdout) => {
        resolve(stdout ? stdout.trim().length > 0 : false);
      });
    } else if (platform === "win32") {
      const psScript = `@(Get-ChildItem -Path "HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone" -Recurse -ErrorAction SilentlyContinue | ForEach-Object { Get-ItemProperty -Path $_.PSPath -Name "LastUsedTimeStop" -ErrorAction SilentlyContinue } | Where-Object { $_ -and $_.LastUsedTimeStop -eq 0 }).Count -gt 0`;
      exec(`powershell -NoProfile -Command "${psScript}"`, (error, stdout) => {
        if (error) return resolve(false);
        resolve(stdout.trim().toLowerCase() === "true");
      });
    } else if (platform === "linux") {
      exec("pactl list source-outputs | grep -q 'Source #'", (error) => {
        resolve(!error);
      });
    } else {
      resolve(false);
    }
  });
});

app.on("window-all-closed", () => {
  if (process.platform === "linux" && !tray) {
    app.quit();
  }
});

// System Media Controls Handler
const MEDIA_COMMANDS = {
  playpause: { linux: "play-pause", win: "TryTogglePlayPauseAsync" },
  next: { linux: "next", win: "TrySkipNextAsync" },
  previous: { linux: "previous", win: "TrySkipPreviousAsync" },
};
ipcMain.handle("control-system-media", async (event, command) => {
  const cmd = MEDIA_COMMANDS[command]; // whitelist: command is spliced into AppleScript below
  if (!trusted(event) || !cmd) return;
  const platform = process.platform;
  if (platform === "darwin") {
    const script = `
        tell application "System Events"
            set spotifyRunning to (name of every process) contains "Spotify"
            set musicRunning to (name of every process) contains "Music"
        end tell
        if spotifyRunning then
            tell application "Spotify" to ${command} track
        else if musicRunning then
            tell application "Music" to ${command} track
        end if
        `;
    exec(`osascript -e '${script}'`);
  } else if (platform === "linux") {
    exec(`playerctl ${cmd.linux}`);
  } else if (platform === "win32") {
    runPs(`Add-Type -AssemblyName System.Runtime.WindowsRuntime; $m = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]::RequestAsync().GetAwaiter().GetResult(); $s = $m.GetCurrentSession(); if ($s) { $s.${cmd.win}().GetAwaiter().GetResult() | Out-Null }`);
  }
});
