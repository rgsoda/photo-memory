# Three ways to build photomem, because they are not interchangeable.
#
# The difference is `release`'s lto = true and codegen-units = 1: they take the
# binary from 20 MB to 11 MB and they are also the two settings that make a
# build serial. Measured on one edit to a string constant, sixteen threads
# either way: 111s release, 10s quick, 1s check. Iterate with `quick`, ship
# `release`.

.DEFAULT_GOAL := quick
.PHONY: release quick check install bundle

PREFIX ?= $(HOME)/.local
BINDIR := $(DESTDIR)$(PREFIX)/bin

# The shipped binary: small and slow to build. What install.sh and the release
# CI produce.
release:
	cargo build --release

# The same optimisations without the serial parts. A different binary in a
# different directory — target/quick/photomem — so it can never be mistaken
# for something to install.
quick:
	cargo build --profile quick

# Does it compile. No binary comes out of this.
check:
	cargo check --profile quick

# Install the release binary and put the running daemon on it.
#
# The restart is the point. photomem is single-instance: the first process to
# start claims a name on the session bus, and every later `photomem capture`
# is handed to *that* process, whichever binary it came from. So a new build
# changes nothing at all until the old daemon dies — it looks exactly like a
# build that silently did not happen.
#
# Only restarted if one was already running, so this never starts a daemon on
# a machine that had chosen not to have one. install.sh is still the thing to
# point a new user at: it checks build dependencies and explains the hotkey.
install: release
	install -Dm755 target/release/photomem $(BINDIR)/photomem
	@if pgrep -x photomem >/dev/null; then \
		pkill -x photomem; \
		sleep 1; \
		setsid $(BINDIR)/photomem daemon >/dev/null 2>&1 </dev/null & \
		echo "restarted the daemon on $(BINDIR)/photomem"; \
	else \
		echo "installed to $(BINDIR)/photomem; no daemon was running"; \
	fi

# A double-clickable macOS .app. Needs the Tauri CLI once: cargo install tauri-cli.
#
# The codesign line is not optional and is also not an Apple account. Two
# different things get confused here: *signing* is required on Apple Silicon,
# where an unsigned arm64 binary will not run at all, and an ad-hoc signature
# ("--sign -", meaning "unaltered since built") satisfies it for free.
# *Notarisation* is the one that costs, and it only matters for an app that
# reaches a Mac carrying a quarantine flag. A local build carries none.
#
# Tauri leaves the bundle with only the linker's own signature on the inner
# binary, which Gatekeeper reads as a bundle whose resources are missing. This
# re-signs the bundle properly.
APP := target/release/bundle/macos/photomem.app

bundle:
	cargo tauri build --bundles app
	codesign --force --deep --sign - $(APP)
	@echo "built and ad-hoc signed $(APP)"
