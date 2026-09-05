//! Reading the text inside a captured screenshot.
//!
//! This is the bet the whole app rests on (DESIGN.md §1): it is what turns a
//! pasted screenshot from decoration into searchable memory. The text goes into
//! the index and never into the note — the markdown stays exactly what was
//! typed, so nothing here can corrupt what is on disk.
//!
//! Two backends, chosen at compile time, the same way the tray and the
//! clipboard already differ. macOS has Vision built in, so it costs no install
//! and reads UI text well. Linux shells out to the `tesseract` binary rather
//! than linking leptonica, so a machine without it degrades to "no OCR" instead
//! of failing to build.

use anyhow::{Context, Result};

/// What one image yielded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognized {
    pub text: String,
    /// The languages that produced this text, joined with `+`.
    ///
    /// Recorded alongside the text so that changing the setting later can find
    /// the rows made under the old one. The images are kept forever, so
    /// re-running is always possible — see `reindex --ocr` in DESIGN.md §3.
    pub languages: String,
}

/// What OCR runs with when the config says nothing.
///
/// Spelled the way tesseract spells it, because that is what DESIGN.md §3
/// settled on and what a Linux install documents. Polish is wanted eventually
/// but adds noise and latency, so it stays a one-line config change.
pub const DEFAULT_LANGUAGES: [&str; 1] = ["eng"];

/// Recognize the text in a stored image.
///
/// Best-effort by nature: an image with no text is not an error, and neither is
/// a machine with no recogniser installed. Callers treat a failure as "no text
/// yet" rather than as something to report.
pub fn recognize(image: &[u8], languages: &[String]) -> Result<Recognized> {
    let langs: Vec<String> = if languages.is_empty() {
        DEFAULT_LANGUAGES.iter().map(|s| s.to_string()).collect()
    } else {
        languages.to_vec()
    };

    let png = to_png(image)?;
    let text = run(&png, &langs)?;
    Ok(Recognized { text: normalize(&text), languages: langs.join("+") })
}

/// Both backends are handed PNG rather than the stored WebP.
///
/// Vision decodes through ImageIO and tesseract through leptonica, and neither
/// is guaranteed to have WebP compiled in. PNG is the one format both certainly
/// read. The cost is re-encoding an image of at most `MAX_EDGE`, on a
/// background thread, which is a better trade than the uncertainty.
fn to_png(image: &[u8]) -> Result<Vec<u8>> {
    let decoded = image::load_from_memory(image).context("decoding a stored image for OCR")?;
    let mut out = std::io::Cursor::new(Vec::new());
    decoded.write_to(&mut out, image::ImageFormat::Png).context("re-encoding for OCR")?;
    Ok(out.into_inner())
}

/// Collapse the whitespace a recogniser leaves behind.
///
/// Both emit a line per detected block and generous blank space between them.
/// The index only ever tokenises this, so the layout carries no information and
/// would otherwise pad every row with runs of newlines.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Vision speaks BCP-47 where the config speaks tesseract's three-letter codes.
///
/// Anything unrecognised is passed through: Vision ignores a language it does
/// not support, which is a better failure than refusing to read the image.
#[cfg(target_os = "macos")]
fn bcp47(code: &str) -> &str {
    match code {
        "eng" => "en-US",
        "pol" => "pl-PL",
        "deu" => "de-DE",
        "fra" => "fr-FR",
        "spa" => "es-ES",
        "ita" => "it-IT",
        "por" => "pt-BR",
        other => other,
    }
}

#[cfg(target_os = "macos")]
fn run(png: &[u8], languages: &[String]) -> Result<String> {
    use objc2::rc::Retained;
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest,
    };

    // SAFETY: every call is a message send to a Vision object created here and
    // owned for the length of this function. Nothing is shared across threads
    // and nothing outlives the call.
    unsafe {
        let data = NSData::with_bytes(png);
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &NSDictionary::new(),
        );

        let request = VNRecognizeTextRequest::new();
        let langs: Vec<Retained<NSString>> =
            languages.iter().map(|l| NSString::from_str(bcp47(l))).collect();
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&langs));

        // Two steps up: VNRecognizeTextRequest sits under VNImageBasedRequest,
        // which sits under the VNRequest the handler takes.
        let as_request: Retained<VNRequest> =
            Retained::into_super(Retained::into_super(request.clone()));
        handler
            .performRequests_error(&NSArray::from_retained_slice(&[as_request]))
            .map_err(|e| anyhow::anyhow!("Vision could not read the image: {e}"))?;

        let Some(results) = request.results() else { return Ok(String::new()) };

        let mut lines = Vec::new();
        for observation in results.iter() {
            // Vision types its results as the base observation; a text request
            // only ever produces text observations.
            let Some(text) = observation.downcast_ref::<VNRecognizedTextObservation>() else {
                continue;
            };
            // One candidate: the alternatives are the same words at lower
            // confidence, and indexing all of them would only dilute the row.
            if let Some(best) = text.topCandidates(1).iter().next() {
                lines.push(best.string().to_string());
            }
        }
        Ok(lines.join("\n"))
    }
}

/// Shelled out to rather than linked.
///
/// `leptess` and friends need leptonica and tesseract headers at build time, so
/// a machine without them cannot compile the app at all — too high a price for
/// a background, index-only feature. The binary reads an image on stdin and
/// writes text on stdout, and if it is missing, OCR is simply unavailable.
#[cfg(not(target_os = "macos"))]
fn run(png: &[u8], languages: &[String]) -> Result<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout", "-l", &languages.join("+")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("running tesseract; install it for OCR")?;

    // Taken and dropped within the statement, which closes the pipe and is what
    // tells tesseract the image is complete. It reads the whole image before
    // writing anything, so filling the stdout buffer cannot deadlock this.
    child
        .stdin
        .take()
        .context("tesseract took no stdin")?
        .write_all(png)
        .context("piping the image to tesseract")?;

    let out = child.wait_with_output().context("waiting for tesseract")?;
    anyhow::ensure!(out.status.success(), "tesseract exited with {}", out.status);
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_the_layout_a_recogniser_emits() {
        assert_eq!(normalize("Kafka\n\n  rebalance   storm \n"), "Kafka rebalance storm");
        assert_eq!(normalize("   \n\n "), "");
    }

    #[test]
    fn an_empty_language_list_falls_back_to_the_default() {
        // A config with `languages = []` must not ask the recogniser for
        // nothing and get nothing back.
        let png = super::to_png(&blank()).unwrap();
        assert!(!png.is_empty());
        let got = recognize(&blank(), &[]).unwrap();
        assert_eq!(got.languages, "eng");
    }

    #[test]
    fn an_image_with_no_text_is_empty_rather_than_an_error() {
        assert_eq!(recognize(&blank(), &["eng".into()]).unwrap().text, "");
    }

    #[test]
    fn records_the_languages_it_was_asked_for() {
        let got = recognize(&blank(), &["eng".into(), "pol".into()]).unwrap();
        assert_eq!(got.languages, "eng+pol");
    }

    /// A flat WebP, as the store would have written it.
    fn blank() -> Vec<u8> {
        let px = vec![255u8; 64 * 64 * 4];
        crate::from_rgba(64, 64, &px, Default::default()).unwrap().webp
    }
}
