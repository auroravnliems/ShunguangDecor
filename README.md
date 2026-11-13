# ShunguangDecor

An all-in-one desktop customizer for Windows 10/11. Everything is configured through the UI, with no hand-written config files.
Each part runs on its own: only the parts you turn on are active, and turning one off puts Windows back the way it was.

| # | Part | What it does |
|---|---|---|
| 01 | YASB Taskbar | Its own YASB-style taskbar, with a Windows button that opens the real Start menu |
| 02 | Live wallpaper | Video, GIF, images, web pages and apps/games as wallpaper; reads Lively wallpapers |
| 03 | Mouse cursor | Install and switch `.ani`/`.cur` cursor schemes in one click, at any size |
| 04 | Desktop labels | Change the color, outline and shadow of desktop icon labels |
| 05 | Hidden features | Turn experimental Windows features on and off (like ViVeTool), with undo |

The app does not touch game processes. Only the **Hidden features** part asks for Administrator rights (a UAC prompt),
and only when you write a change.

## Download and install

Download `ShunguangDecor_<version>_x64-setup.exe` from **Releases** and run it. The installer:

- installs for your account only (`%LOCALAPPDATA%\Programs\ShunguangDecor`), with no Administrator rights needed;
- supports 10 languages and creates a Start menu shortcut (the Desktop shortcut is chosen on the last page);
- on uninstall: restores your cursor, wallpaper, desktop labels and native taskbar, and removes the start-with-Windows
  entry; a checkbox lets you also delete the app's settings. Changes made in Hidden features are not undone
  automatically on uninstall (the uninstaller reminds you to undo them in the app first).

The app is not code-signed yet, so on first run Windows SmartScreen may show "Windows protected your PC": click
**More info → Run anyway**.

### Data and portable mode

Settings live in `%APPDATA%\ShunguangDecor`; logs and temporary files live in `%LOCALAPPDATA%\ShunguangDecor`. To carry
the app on a USB drive, put an empty file named `portable.txt` next to `ShunguangDecor.exe`: everything is then stored in
a `data` folder right beside it.

## YASB Taskbar (part 01)

Its own taskbar in the style of [YASB](https://github.com/amnweb/yasb), configured entirely through the UI (no YAML/CSS
to write): top or bottom edge, height, flush or floating, color, opacity, Blur/Acrylic, font, and widgets in three zones
(Left/Center/Right; drag and drop, double-click to edit one). It has a **Windows button** that opens the real Start
menu, the Windows **Search box**, open apps (hover to see window thumbnails), the system tray, clock, volume, network,
battery and notifications.

- **Native Windows taskbar:** Keep / Hide completely / **Hide, show on edge touch** (default). When hidden, the app turns
  on the native taskbar's auto-hide so Windows gives the screen space back, and shows it again as soon as the mouse
  touches the edge. Turning the part off, quitting the app, or the app being killed all restore the native taskbar and
  its auto-hide setting.
- **Hot corner** (off by default): touching the chosen screen corner opens Start.
- **Native config files:** the app generates YASB-style `config.yaml` + `styles.css`; you can open the folder, edit them
  by hand, or **import an existing YASB theme**. If the files have been edited by hand, the app asks before
  overwriting them.

This part is based on TachyonTaskbar (a rewrite of YASB in pure Rust, Direct2D/DirectWrite), MIT, which is in turn based
on YASB (MIT).

## Live wallpaper (part 02)

The wallpaper sits **behind the desktop icons** like a regular background. The **Wallpaper** page has: a library
(drag and drop files, paste a URL, import a Lively folder/`.zip`), per-monitor / span / duplicate modes, fit modes,
volume, the player (Media Foundation built into Windows, or mpv if you install it), **auto-pause** rules (a covering
fullscreen app/game, running on battery, Battery Saver, Remote Desktop, an app list) and the **Customize** panel for
Lively wallpapers. Turning this part off restores the Windows wallpaper exactly as it was. The preview panes on the
pages show your real desktop (captured in memory, never written to disk, and only while the page is open).

This part was ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).

## Mouse cursor (part 03)

Import a cursor scheme from a `.zip`, a folder or `install.inf`; the app detects the role of each file automatically
(you can adjust it by hand) and previews animated cursors too. Pick a size from 32 to 256 px and a scaling style,
**Sharp** (keeps pixels) or **Smooth**: a size change applies immediately, with no sign-out needed. Turning the part off
brings back your original cursor scheme and cursor size.

## Desktop labels (part 04)

Change the color of desktop icon labels: one color for all, or **Auto**, which picks a readable color from the
wallpaper area behind each label (also while a live wallpaper is running), plus outline, shadow and a translucent
backdrop. This part uses a small DLL inside Explorer; if the app exits or crashes, the DLL restores the labels to the
Windows default by itself.

## Hidden features (part 05)

A Rust port of [ViVe/ViVeTool](https://github.com/thebookisclosed/ViVe): view, search by name/ID, browse all features on
the machine, enable/disable/reset to default in the Runtime or Boot store, and export/import configurations. Every write
is recorded in a journal so you can **undo everything** with one button. Reading needs no admin rights; writing goes
through a helper process that runs after a UAC prompt.

## Build from source

You need Windows 10/11, Node 24+ and Rust stable with the MSVC toolchain (`rust-toolchain.toml` already selects it).

```
npm ci
npm run hook           # build the Explorer DLL (shunguang_explorer.dll) — the app embeds it at build time
npm run tauri dev      # run in development, reloads automatically when the UI changes
npx tauri build        # NSIS installer in target/release/bundle/nsis/ (runs `npm run hook` and builds the UI itself)
```

Before sending a change, run:

```
npm run typecheck
npm test
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Contributing translations

The UI is available in 10 languages: Tiếng Việt, English, Русский, 简体中文, 한국어, 日本語, Deutsch, Español,
Português (Brasil), Français. The source language is Vietnamese; the other eight languages (apart from Vietnamese and
English) were translated by AI, so native speakers are very welcome to polish them.

Each language is one `src/locales/<code>.json` file. Edit the strings, keep the keys and the `{placeholders}` unchanged,
then run `npm test` (the tests report missing keys, missing plural forms or wrong `{placeholders}`) and open a pull
request.

## Reporting bugs

In the app go to **Settings → Support → Export bug report**, then open an issue using the template, or send it on
Discord.

## License and credits

ShunguangDecor is licensed under **GPL-3.0-or-later** (see `LICENSE`), because it uses code and ideas from the
open-source projects below. Thanks to the authors of:

- [Lively Wallpaper](https://github.com/rocksdanister/lively) — GPL-3.0 (the idea of attaching the wallpaper under the icons, the `LivelyInfo.json`/`LivelyProperties.json` formats)
- TachyonWallpaper — GPL-3.0 (the wallpaper engine rewritten in Rust, now the `sg-wallpaper` crate)
- [TranslucentTB](https://github.com/TranslucentTB/TranslucentTB) — GPL-3.0 (how to load a DLL into Explorer's desktop thread with a hook)
- [ViVe](https://github.com/thebookisclosed/ViVe) — GPL-3.0 (author thebookisclosed; the Hidden features part, the `sg-features` crate, ports the logic of ViVe/ViVeTool to Rust; the file `crates/sg-features/data/FeatureDictionary.pfs` is taken byte for byte from ViVe)
- [YASB](https://github.com/amnweb/yasb) — MIT
- TachyonTaskbar — MIT (the taskbar engine, a rewrite of YASB in Rust, now `crates/sg-taskbar/src/engine`)

Third-party cursor schemes are not bundled with the app. Install only schemes you have the right to use.

## Community

Discord: https://discord.gg/k55b6F3h2z
