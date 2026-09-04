# photomem

Fast visual note capture. Hotkey, type, save, gone.

See [DESIGN.md](DESIGN.md) for the format and the plan. Working today: capture, clipboard
images, full-text search, the read-only viewer, citations between notes, git sync, and the
thumbnail wall, timeline and tags — on Linux and macOS both. Not yet: OCR and a packaged macOS build.

## Install

```bash
git clone git@github.com:rgsoda/photo-memory.git
cd photo-memory
./install.sh
```

That checks the build dependencies, builds, and puts the binary in `~/.local/bin`. It never
touches your compositor config or your shell profile — it prints what to add and leaves
those files to you. It is Linux-only; on macOS, build by hand as below.

| | |
|---|---|
| `./install.sh --check` | report what is missing, build nothing |
| `./install.sh --deps` | install the system packages first (uses `sudo`) |
| `./install.sh --prefix DIR` | install to `DIR/bin` instead |

### Or by hand

Needs Rust and, on Linux, `webkit2gtk-4.1`, `gtk3`, `libsoup3` and `librsvg`. There is no
npm step; the UI is plain HTML in `ui/` and is embedded into the binary at build time.

```bash
cargo build --release
install -Dm755 target/release/photomem ~/.local/bin/photomem
```

The one dependency that matters is the webview: Tauri renders the UI with WebKitGTK rather
than shipping a browser, which is what keeps the binary around 11 MB.

## Configure

`~/.config/photomem/config.toml`, written with defaults on first run:

```toml
# The notes repository. Created on first save; make it a git repo to sync it.
vault = "/home/you/photomem"

# The global capture hotkey. macOS only — on Linux this is a compositor binding.
# Modifiers: Ctrl, Alt, Shift, Cmd (or Super). Takes effect on restart.
hotkey = "Ctrl+Alt+N"

[sync]
# Commit each saved note to the vault's git repo, and push it if that repo has
# a remote. Does nothing until the vault is a git repo, so `git init` there is
# what actually turns sync on.
enabled = true

[image]
# Pasted images are scaled to this long edge and re-encoded as WebP. Measured on
# a 4K screenshot: 1200 is 62 KB but too soft to read, 1600 is 104 KB and legible,
# 1920 is 145 KB and sharp. Window captures are usually below this and untouched.
max_edge = 1600
thumb_edge = 320
quality = 75
```

The vault is a **separate repo** from this one — this holds the app, that holds your notes.

## Run it from a hotkey

`photomem daemon` starts hidden and stays warm; `photomem capture` wakes it. The first
capture is then as fast as every later one.

**Hyprland** — Wayland has no protocol for app-registered global hotkeys, so the binding
lives in the compositor.

Stock Hyprland, in `hyprland.conf`:

```
exec-once = photomem daemon
bind = SUPER, N, exec, photomem capture
windowrulev2 = float, class:^(photomem)$
windowrulev2 = center, class:^(photomem)$
```

Omarchy configures Hyprland in **Lua**, so the same thing is spread across three files:

```lua
-- ~/.config/hypr/autostart.lua
o.launch_on_start("photomem daemon")

-- ~/.config/hypr/bindings.lua
o.bind("SUPER + N", "Photo memory", "photomem capture")

-- ~/.config/hypr/hyprland.lua
-- Both windows float and centre...
o.window({ class = "^photomem$" }, { float = true, center = true })
-- ...but only the capture window has a fixed size. The image window sizes itself
-- to the picture, and a class-wide size rule would override that — and a
-- title-scoped float rule would leave the image window tiling.
o.window({ class = "^photomem$", title = "^photomem$" }, { size = "720 400" })
```

Validate with `hyprctl reload && hyprctl configerrors`.

**i3 / sway** — in `~/.config/i3/config`:

```
exec --no-startup-id photomem daemon
bindsym $mod+n exec --no-startup-id photomem capture
for_window [class="photomem"] floating enable, move position center
for_window [class="photomem" title="^photomem$"] resize set 720 400
```

The second rule is narrower than the first on purpose: the image window sizes itself to
the picture, so a class-wide `resize` would override it. Reload with `i3-msg reload`.

On sway the file is `~/.config/sway/config`, the reload is `swaymsg reload`, and windows
match on `app_id` rather than `class` — Wayland has no `WM_CLASS`.

**KDE Plasma** — Autostart and the shortcut both live in System Settings: *Autostart →
Add → Application → `photomem daemon`*, and *Keyboard → Shortcuts → Add New → Command or
Script → `photomem capture`*, bound to Meta+N. The autostart entry is just a file, if you
would rather write it — `~/.config/autostart/photomem.desktop`:

```ini
[Desktop Entry]
Type=Application
Name=photomem daemon
Exec=photomem daemon
```

**GNOME** — same autostart file. The hotkey is a custom shortcut under *Settings →
Keyboard → View and Customize Shortcuts → Custom Shortcuts*, or from a terminal:

```bash
k=org.gnome.settings-daemon.plugins.media-keys
p=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/photomem/
gsettings set $k custom-keybindings "['$p']"          # replaces the whole list
gsettings set $k.custom-keybinding:$p name 'photomem'
gsettings set $k.custom-keybinding:$p command 'photomem capture'
gsettings set $k.custom-keybinding:$p binding '<Super>n'
```

That third line replaces every custom shortcut you have. If you already have some, append
to what `gsettings get $k custom-keybindings` returns.

Neither KDE nor GNOME needs window rules: KWin and Mutter float the window and honour the
size it asks for. The Hyprland and i3 rules exist only because a tiling compositor does
neither.

`./install.sh` prints whichever of these applies to the desktop you are running.

**macOS** — nothing to configure in a compositor: the app registers the hotkey itself.
Start `photomem daemon` and press **`Ctrl+Alt+N`**.

The binding lives in `config.toml` rather than being fixed, because a global hotkey that
collides with something you already use would otherwise leave the app unreachable. It is
not `Super+N` as on Linux — "Super" is Command on macOS, and grabbing Cmd+N globally would
swallow "New" in every application on the machine.

Registration goes through Carbon's `RegisterEventHotKey`, which asks for no accessibility
permission, so there is no prompt on first run and nothing to grant in System Settings. If
the key cannot be bound the app says so on stderr and starts anyway — `photomem capture`
still opens the window.

Not packaged yet: no `.app`, no menu bar agent, no LaunchAgent, so the daemon keeps a dock
icon and is started by hand. That is the rest of M8.

## The status bar icon

photomem lives hidden behind a hotkey, which makes it an app you cannot tell is running,
cannot reach when you have forgotten the binding, and cannot quit. The tray icon is the
daemon's entire visible surface: **left click captures**, right click opens a menu with
*New note* and *Quit photomem*.

On Linux the click does nothing — the StatusNotifierItem hosts reached through
libappindicator deliver no click events at all — so the menu is the whole interaction
there, which is why *New note* is in it rather than being click-only. Omarchy's bar keeps
the tray collapsed behind the `<` chevron on the right; the icon is in there.

The artwork is `src-tauri/icons/tray/photomem.svg`, drawn on a 16-unit grid because 16px is
the size that has to survive. `render.sh` produces the two PNGs the binary embeds: black
for macOS, which treats it as a template image and recolours it to match the menu bar, and
white for Linux, where nothing recolours anything and the icon has to arrive the right
colour for the bar. Edit the SVG, run the script, commit both.

## Keys

| Key | Action |
|---|---|
| *(type)* | First non-empty line becomes the title and the filename slug |
| `Ctrl+V` | Paste an image from the clipboard |
| `//` | In an empty buffer, switch to search |
| `//` or `[[` | Mid-note, opens the picker to cite another note |
| `#` | At the start of a word, completes from the tags already in use |
| `Ctrl+Enter` | Save and hide |
| `Esc` | Hide, stashing the text as a draft |

In search: `↑`/`↓` move, `Enter` opens the note read-only, `Esc` goes back. `Tab` cycles
through the two browse views and back — search, wall, timeline.

On the **wall** — every picture you have ever captured, newest first: arrows move in two
dimensions, `Enter` opens the note the picture belongs to, `V` opens the picture itself and
`←`/`→` there step through the whole wall rather than one note.

On the **timeline** — every note by date: `↑`/`↓` and `PageUp`/`PageDown` move, `Enter`
opens, and `G` cycles the grouping between day, week and month without losing your place.

`T` filters either view to one tag, and the filter follows you between them. `Esc` clears
the filter first and leaves the view second.

`Esc` returns to search from either.

In the citation picker: `↑`/`↓` move, `Enter` inserts `[[that-note]]` at the cursor,
`Ctrl+Enter` inserts it as a `supersedes:` line instead, and `Esc` cancels and leaves the
literal `//` behind. A trigger only fires at the start of a word, so `https://` and a pasted
`// comment` are ordinary text.

Citations render as links in the read-only view: `Tab` reaches them and `Enter` follows.

An open note shows what links to it under "Linked from", and one that a later note declares
it `supersedes:` opens with a banner naming the correction — `Tab` reaches both and `Enter`
follows. Where several notes correct the same one the banner names the newest, and the
others stay in the list rather than disappearing.

In a note: `↑`/`↓` (or `j`/`k`) scroll a line, `PageUp`/`PageDown` and `Space` scroll a
screen, `Home`/`End` jump to either end, and `Tab` reaches its `[[links]]` with `Enter` to
follow. `V` opens its image in its own window, as does clicking any thumbnail. That
window is always 1600px on its long edge in the image's aspect ratio, so a full-screen
capture opens at its own size and a smaller image is scaled up into the same frame. `Z` (or
a click) toggles fit and 1:1, centred either way — drag to pan when it overflows — `←`/`→`
step through the note's images, and `Esc` closes it. Notes are never
edited in the app — corrections are new notes that supersede old ones, and a typo is fixed
by opening the file in any editor.

Accents can be skipped while typing: `gesla` finds `gęślą`. The one exception is `ł`, which
has no Unicode decomposition and so does not fold — `zazołc` finds `zażółć`, `zazolc` does not.

Escape never loses text. The buffer is written to `.photomem/draft.md` and comes back on
the next capture, cursor at the end.

## What lands on disk

```
vault/
  notes/2026-09-04-1652-kafka-rebalance-storm.md
  attachments/2026-09-04-e382057b79ca.webp
  .photomem/                   # gitignored: index, thumbnails, drafts
  .gitignore
```

```markdown
---
id: "01M1PEG62DF74YR1GC3A3DTW5A"
created: "2026-09-04T16:52:25+02:00"
modified: "2026-09-04T16:52:25+02:00"
---
Kafka rebalance storm
![[2026-09-04-e382057b79ca.webp]]
Consumers dropped every 40s.
```

Images are content-hashed, so pasting the same screenshot twice stores one file. They are
never edited and never deleted, which is what keeps them cheap in git.

Notes are plain markdown and nothing but the app cares about the filename, so editing one
in any editor is safe and expected — that is the escape hatch that makes read-only,
append-only capture reasonable.

## Working on it

The UI is embedded in the binary at build time. `src-tauri/build.rs` declares `ui/` as a
build input, without which cargo happily relinks the old frontend into a "rebuilt" binary
and every change to the page appears to do nothing.

Build with `--profile quick` while iterating:

```bash
make quick && ./target/quick/photomem capture
```

`make` on its own does that. `make check` compiles without producing a binary, `make
release` builds the shipped one, and `make install` puts it in `~/.local/bin` and restarts
a running daemon onto it.

That restart matters more than it sounds. photomem is single-instance: the first process
to start claims a name on the session bus, and every later `photomem capture` is handed to
*that* process, whatever binary it came from. Until the old daemon dies a new build changes
nothing, and it looks exactly like a build that silently did not happen.

`release` sets `lto = true` and `codegen-units = 1` to get the binary from 20 MB to 11 MB.
Both are worth it in a shipped binary and neither is worth it in a rebuild loop: they are
the two settings that make a build serial, so a one-line change costs 111s under `release`
and 10s under `quick` on the same sixteen threads. `cargo check` is a second, if all you
need is whether it compiles.

## Not there yet

- No tray icon, so the daemon is stopped with `pkill -x photomem`.
- Images show in a strip under the editor, not inline in the text.
- Search has no tag or date filters yet.
- On macOS the app runs but is not packaged: no `.app` bundle, menu bar agent or
  LaunchAgent, and `install.sh` is Linux-only (M8).
- Tags are inline text, so `#ff0000` in a note about CSS becomes a tag. `#1234` and
  `# heading` do not.
- No OCR (M6).
