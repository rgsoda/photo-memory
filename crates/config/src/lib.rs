//! Configuration, read from `~/.config/photomem/config.toml` on both platforms.
//!
//! macOS convention would be `~/Library/Application Support`, but a config file
//! is something you edit by hand, and `~/.config` is where it can be found.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The notes repository. A separate git repo from the app itself.
    pub vault: PathBuf,
    pub image: ImageConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config { vault: home().join("photomem"), image: ImageConfig::default() }
    }
}

/// How pasted images are stored. Defaults suit a 4K screen; someone on a 1080p
/// laptop can drop `max_edge` and halve their repo.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ImageConfig {
    /// Long edge in pixels. Smaller images are never upscaled.
    pub max_edge: u32,
    /// Long edge of grid thumbnails.
    pub thumb_edge: u32,
    /// WebP quality, 0-100.
    pub quality: f32,
}

impl Default for ImageConfig {
    fn default() -> Self {
        let d = photomem_images::Options::default();
        ImageConfig { max_edge: d.max_edge, thumb_edge: d.thumb_edge, quality: d.quality }
    }
}

impl From<ImageConfig> for photomem_images::Options {
    fn from(c: ImageConfig) -> Self {
        photomem_images::Options {
            max_edge: c.max_edge.max(64),
            thumb_edge: c.thumb_edge.max(32),
            quality: c.quality.clamp(1.0, 100.0),
        }
    }
}

/// The template written on first run, so the file is discoverable and
/// self-documenting rather than an empty mystery.
const TEMPLATE: &str = "\
# photomem configuration
# https://github.com/soda/photo-memory

# The notes repository. Created on first save; make it a git repo to sync it.
vault = \"{vault}\"

[image]
# Pasted images are scaled to this long edge and re-encoded as WebP. Measured on
# a 4K screenshot: 1200 is 62 KB but too soft to read, 1600 is 104 KB and legible,
# 1920 is 145 KB and sharp. Window captures are usually below this and untouched.
max_edge = {max_edge}
thumb_edge = {thumb_edge}
quality = {quality}
";

impl Config {
    /// Load the config, writing a commented default file if none exists.
    pub fn load() -> Result<Config> {
        Config::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Result<Config> {
        if !path.exists() {
            let cfg = Config::default();
            cfg.write_template(path)
                .with_context(|| format!("writing default config to {}", path.display()))?;
            return Ok(cfg);
        }

        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        cfg.vault = expand_tilde(&cfg.vault);
        Ok(cfg)
    }

    fn write_template(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = TEMPLATE
            .replace("{vault}", &self.vault.display().to_string())
            .replace("{max_edge}", &self.image.max_edge.to_string())
            .replace("{thumb_edge}", &self.image.thumb_edge.to_string())
            .replace("{quality}", &self.image.quality.to_string());
        std::fs::write(path, body)?;
        Ok(())
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"));
    base.join("photomem").join("config.toml")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn expand_tilde(p: &Path) -> PathBuf {
    match p.strip_prefix("~") {
        Ok(rest) => home().join(rest),
        Err(_) => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("photomem-cfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writes_a_readable_default_when_missing() {
        let path = tmpdir("default").join("config.toml");
        let cfg = Config::load_from(&path).unwrap();

        assert_eq!(cfg, Config::default());
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("vault ="));
        // The template must round-trip through the parser it documents.
        assert_eq!(Config::load_from(&path).unwrap(), cfg);
    }

    #[test]
    fn expands_tilde_in_vault() {
        let path = tmpdir("tilde").join("config.toml");
        std::fs::write(&path, "vault = \"~/notes\"\n").unwrap();
        assert_eq!(Config::load_from(&path).unwrap().vault, home().join("notes"));
    }

    #[test]
    fn image_defaults_are_filled_in_when_absent() {
        let path = tmpdir("image-absent").join("config.toml");
        std::fs::write(&path, "vault = \"~/notes\"\n").unwrap();
        assert_eq!(Config::load_from(&path).unwrap().image, ImageConfig::default());
    }

    #[test]
    fn image_settings_override_and_clamp() {
        let path = tmpdir("image-set").join("config.toml");
        std::fs::write(&path, "[image]\nmax_edge = 1920\nquality = 200\n").unwrap();
        let opts: photomem_images::Options = Config::load_from(&path).unwrap().image.into();
        assert_eq!(opts.max_edge, 1920);
        // A nonsense quality must not reach the encoder.
        assert_eq!(opts.quality, 100.0);
    }

    #[test]
    fn rejects_unknown_keys_loudly() {
        let path = tmpdir("unknown").join("config.toml");
        std::fs::write(&path, "vualt = \"~/notes\"\n").unwrap();
        // A typo silently ignored means notes quietly landing in the wrong place.
        assert!(Config::load_from(&path).is_err());
    }
}
