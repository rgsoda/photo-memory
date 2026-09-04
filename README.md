# photomem

Fast visual note capture. Hotkey, type, save, gone.

See [DESIGN.md](DESIGN.md) for the format and the plan. This is **M1**: capture only —
no images, no search, no sync yet.

## Build

Needs Rust and, on Linux, `webkit2gtk-4.1`, `gtk3` and `libsoup3`. There is no npm step;
the UI is plain HTML in `ui/`.

```bash
cargo build --release
install -Dm755 target/release/photomem ~/.local/bin/photomem
```

## Configure

`~/.config/photomem/config.toml`, written with defaults on first run:

```toml
# The notes repository. Created on first save; make it a git repo to sync it.
vault = "/home/you/photomem"
```

The vault is a **separate repo** from this one — this holds the app, that holds your notes.

## Run it from a hotkey

`photomem daemon` starts hidden and stays warm; `photomem capture` wakes it. The first
capture is then as fast as every later one.

**Hyprland** — Wayland has no protocol for app-registered global hotkeys, so the binding
lives in the compositor:

```
exec-once = photomem daemon

bind = SUPER, N, exec, photomem capture

windowrulev2 = float, class:^(photomem)$
windowrulev2 = center, class:^(photomem)$
windowrulev2 = stayfocused, class:^(photomem)$
```

**macOS** — not yet; see M8 in the design. The app registers its own hotkey there.

## Keys

| Key | Action |
|---|---|
| *(type)* | First non-empty line becomes the title and the filename slug |
| `Ctrl+Enter` | Save and hide |
| `Esc` | Hide, stashing the text as a draft |

Escape never loses text. The buffer is written to `.photomem/draft.md` and comes back on
the next capture, cursor at the end.

## What lands on disk

```
vault/
  notes/2026-09-04-1652-kafka-rebalance-storm.md
  attachments/                 # M2
  .photomem/                   # gitignored: drafts now, index and thumbs later
  .gitignore
```

```markdown
---
id: "01M1PEG62DF74YR1GC3A3DTW5A"
created: "2026-09-04T16:52:25+02:00"
modified: "2026-09-04T16:52:25+02:00"
---
Kafka rebalance storm
Consumers dropped every 40s.
```

Notes are plain markdown and nothing but the app cares about the filename, so editing one
in any editor is safe and expected — that is the escape hatch that makes read-only,
append-only capture reasonable.

## Not there yet

- No tray icon, so the daemon is stopped with `pkill -x photomem`.
- No image paste (M2), search (M3), links (M4), sync (M5), OCR (M6).
