# photomem

Fast visual note capture. Hotkey, type, save, gone.

See [DESIGN.md](DESIGN.md) for the format and the plan. Working today: capture, clipboard
images, full-text search, the read-only viewer, citations between notes, git sync, and the
thumbnail wall, timeline, tags and OCR — on Linux and macOS both. Not yet: a packaged
macOS build.

## Install

### Homebrew, on macOS

```bash
brew tap rgsoda/photomem https://github.com/rgsoda/photo-memory
brew install --HEAD rgsoda/photomem/photomem
brew services start photomem
```

`brew services` writes and loads the LaunchAgent, so the daemon starts at login and there
is no plist to write by hand.

`--HEAD` is needed because there is no tagged release yet: the formula builds from `main`.
That is deliberate rather than an oversight — a formula pinning a stable version at a
moving branch would leave `brew upgrade` convinced it was already current. `brew upgrade
--fetch-HEAD photomem` picks up new commits.

### From source, on Linux or macOS

```bash
git clone git@github.com:rgsoda/photo-memory.git
cd photo-memory
./install.sh
```

That checks the build dependencies, builds, and puts the binary in `~/.local/bin`. It never
touches your compositor config or your shell profile — it prints what to add and leaves
those files to you. It works on macOS too, where the only system dependency is the
Xcode Command Line Tools.

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

[ocr]
# Text inside pasted screenshots is read in the background and put into the
# search index only — never into the note. Named as tesseract names them.
# Adding a language makes every image read under the old list stale, and they
# are re-read in the background, since the pictures are kept forever.
languages = ["eng"]
```

macOS reads them with Vision, which is part of the OS. Linux shells out to `tesseract`,
so install it (`pacman -S tesseract tesseract-data-eng`, or your distro's equivalent) —
without it everything else still works and only OCR is missing.

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

The daemon runs as a menu bar agent, so it has no dock icon and does not appear in
Cmd-Tab — it is something you summon, not something you switch to.

To start it at login, `./install.sh` prints a `~/Library/LaunchAgents` plist to write and
the `launchctl load` line for it. As with the compositor config on Linux, it prints and
leaves the file to you.

### A double-clickable app, without an Apple account

`make bundle` produces `target/release/bundle/macos/photomem.app`, ad-hoc signed. It needs
the Tauri CLI once: `cargo install tauri-cli`.

No Apple Developer account is involved, and none is needed. Two things get run together
here and they are not the same:

- **Signing** is required on Apple Silicon — an unsigned arm64 binary will not run at all.
  An *ad-hoc* signature (`codesign --sign -`) satisfies that, costs nothing and needs no
  account. `make bundle` does it. It is not decoration: Tauri leaves the bundle carrying
  only the linker's signature on the inner binary, which Gatekeeper reads as a bundle
  whose resources have gone missing.
- **Notarisation** is the part that needs the paid account, and it only matters for an app
  that reaches a Mac with a quarantine flag on it — which is what a browser download,
  AirDrop or a mail attachment sets.

So an app you built on the machine you run it on has no quarantine flag and opens
normally. `spctl -a` will still say `rejected`; that is it reporting the app is not
notarised, not a prediction that it will refuse to open.

If you do carry the `.app` to another Mac it will be quarantined there and Gatekeeper will
block it. Either strip the flag:

```bash
xattr -dr com.apple.quarantine /Applications/photomem.app
```

or, the first time only, right-click the app and choose **Open** — that dialog has a button
the double-click one does not.

For something that lives in the menu bar and answers a hotkey, `./install.sh` plus the
LaunchAgent is honestly the better route. The bundle is there for when you want it in
Applications.

## The status bar icon

photomem lives hidden behind a hotkey, which makes it an app you cannot tell is running,
cannot reach when you have forgotten the binding, and cannot quit. The tray icon is the
daemon's entire visible surface: **left click captures**, right click opens a menu with
*New note* and *Quit photomem*.

Once anything has synced, the menu also carries a line saying what it did and when —
`synced · 14:23`, or `⚠ sync failed: … · 14:23` — and the same text becomes the icon's
tooltip. It is deliberately quiet: a failed push is something to notice on your own time,
not a dialog in front of a half-typed note. There is no line at all until a sync has
happened, because a vault that is not a git repo never will, and a standing "not synced"
would be nagging about a deliberate choice.

This works the same on both, which took two implementations. Tauri's tray goes through
libappindicator on Linux, and libappindicator exports a StatusNotifierItem `Activate`
method with nothing registered behind it: a left click reaches the process and dies there
with "no handler for Activate". The bar is doing its part — there is simply nothing
listening. So Linux talks StatusNotifierItem directly, through ksni, and handles `Activate`
itself. That also drops a system dependency the build had quietly acquired, since linking
libappindicator needs ayatana-appindicator3 installed; ksni needs nothing.

Omarchy's bar keeps the tray collapsed behind the `<` chevron on the right; the icon is in
there.

## Icons

Two drawings, in `src-tauri/icons/`. `tray/photomem.svg` is the status bar glyph, on a
16-unit grid because 16px is the size that has to survive. `photomem.svg` is the app icon:
the same mark given a ground and a container, on Apple's 1024 grid with an 824 tile so it
sits at the same visual size as stock macOS icons rather than looming over them.

`icons/render.sh` produces everything either one needs — edit an SVG, run the script,
commit the output. The PNGs are committed because the binary embeds them and a build
machine should not need a renderer.

The tray glyph renders twice on purpose: black for macOS, which treats it as a template
image and recolours it to match the menu bar, and white for Linux, where nothing recolours
anything and the pixmap has to arrive already the right colour. The app icon is white on
blue rather than blue on ink because at 16px the low-contrast version turns into a smudge.

`icon.icns` is assembled by the script rather than by `iconutil`, which is macOS-only —
modern .icns entries are PNGs with a four-byte type in front. It has been checked
structurally but never opened by Finder.

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

Typing `#work` in the search box narrows to notes carrying that tag, and it includes
everything beneath it — `#work` finds a note tagged `#work/kafka`. Several tags narrow
rather than widen. A `#404` is treated as text, since that is a number people write in
prose rather than a filter.

Dates narrow it too. `since:` is inclusive and `before:` is exclusive, and both take a
day, a month, a year, or a distance back from today:

| | |
|---|---|
| `since:2026-03-11` | from that day |
| `since:2026-03` | from the first of March — a month names its first day, so `before:2026-03` is "up to March" |
| `since:2026` | from the first of January |
| `since:today`, `since:yesterday` | |
| `since:7d`, `since:2w`, `since:3m` | days, weeks and calendar months back |

They combine with each other and with tags: `since:2026-06 before:2026-07 #work kafka` is
a window. Anything that cannot be read as a date — `since:soon` — is searched for as
ordinary text rather than silently matching nothing, the same rule `#404` follows.

Search covers the text *inside* your screenshots as well as the notes themselves, so a
capture of a stack trace is findable by a line in it. That text lives only in the index,
never in the markdown. It is read in the background after a save and on every window open,
so a just-pasted image becomes searchable a moment later rather than holding up the save.

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

- Images show in a strip under the editor, not inline in the text.
- The macOS `.app` is ad-hoc signed, not notarised, so it is fine on machines you build on
  but Gatekeeper stops it on any Mac it is copied to until the quarantine flag is removed.
  Notarising needs a paid Apple account (M8).
- Tags are inline text, so `#ff0000` in a note about CSS becomes a tag. `#1234` and
  `# heading` do not.
