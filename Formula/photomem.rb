# Installed as a tap rather than through homebrew-core, which will not take a
# formula with no tagged release and no users. See README.md.
class Photomem < Formula
  desc "Fast visual note capture: hotkey, type, paste a screenshot, done"
  homepage "https://github.com/rgsoda/photo-memory"
  license "MIT"

  # HEAD-only on purpose. There is no tagged release yet, and pinning a stable
  # `url` at a moving branch would make `brew upgrade` believe it was already
  # current. When there is a tag, add a `url`/`sha256` stanza and this becomes
  # a plain `brew install`.
  head "https://github.com/rgsoda/photo-memory.git", branch: "main"

  depends_on "rust" => :build
  # Linux needs webkit2gtk, gtk3, libsoup3 and librsvg from the distribution
  # rather than from brew; install.sh knows the package names per distro.
  depends_on :macos

  def install
    system "cargo", "install", *std_cargo_args(path: "src-tauri")
  end

  # `brew services start photomem` writes and loads the LaunchAgent that the
  # README otherwise asks you to write by hand.
  service do
    run [opt_bin/"photomem", "daemon"]
    keep_alive true
    log_path var/"log/photomem.log"
    error_log_path var/"log/photomem.log"
  end

  def caveats
    <<~EOS
      The capture hotkey defaults to Ctrl+Alt+N and is registered by the app
      itself, so there is nothing to grant in System Settings.

      Start it in the background with:
        brew services start photomem

      First run writes ~/.config/photomem/config.toml. Set `vault` in it to
      where your notes should live, and make that directory a git repo if you
      want them synced.
    EOS
  end

  test do
    assert_match "photomem", shell_output("#{bin}/photomem --help")
  end
end
