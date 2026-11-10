//! Downloads, installs and removes models on disk.
//!
//! Layout under the models directory:
//!
//! ```text
//! models/
//!   supertonic-3/            an installed model (the archive's contents)
//!     .speachy-model.json    written last; its presence means "installed"
//!   .downloads/              partial downloads, kept for resuming
//! ```

use crate::catalog::{self, ModelInfo};
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const MARKER: &str = ".speachy-model.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Downloading,
    Extracting,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub done_bytes: u64,
    pub total_bytes: u64,
}

/// Raised when a download is cancelled; partial data is kept for resuming.
#[derive(Debug, thiserror::Error)]
#[error("download cancelled")]
pub struct Cancelled;

#[derive(Clone)]
pub struct ModelManager {
    root: PathBuf,
}

impl ModelManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn model_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.model_dir(id).join(MARKER).is_file()
    }

    /// Installed models, in catalog order.
    pub fn installed(&self) -> Vec<&'static ModelInfo> {
        catalog::catalog()
            .iter()
            .filter(|m| self.is_installed(&m.id))
            .collect()
    }

    /// Bytes of a partial download waiting to be resumed.
    pub fn partial_bytes(&self, model: &ModelInfo) -> u64 {
        fs::metadata(self.part_path(&model.id)).map_or(0, |m| m.len())
    }

    fn downloads_dir(&self) -> PathBuf {
        self.root.join(".downloads")
    }

    fn part_path(&self, id: &str) -> PathBuf {
        self.downloads_dir().join(format!("{id}.tar.bz2.part"))
    }

    /// Downloads and installs `model`. Safe to call again after a failure
    /// or cancellation: the archive download resumes where it stopped.
    pub fn install(
        &self,
        model: &ModelInfo,
        cancel: &AtomicBool,
        progress: &dyn Fn(Progress),
    ) -> Result<()> {
        if self.is_installed(&model.id) {
            return Ok(());
        }
        fs::create_dir_all(self.downloads_dir())?;
        let extras: u64 = model.extra_files.iter().map(|f| f.size_bytes).sum();
        let total = model.size_bytes;
        let archive_total = total.saturating_sub(extras);

        let part = self.part_path(&model.id);
        download(&model.archive_url, &part, cancel, &|done, len| {
            let total = len.map_or(total, |len| len + extras);
            progress(Progress {
                phase: Phase::Downloading,
                done_bytes: done,
                total_bytes: total,
            });
        })
        .with_context(|| format!("downloading {}", model.name))?;

        // Unpack next to the final location, then move into place, so a
        // half-extracted model is never mistaken for an installed one.
        let staging = self.root.join(format!(".{}.staging", model.id));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;
        let result = (|| -> Result<()> {
            let archive_len = fs::metadata(&part)?.len();
            let counted = CountingReader {
                inner: File::open(&part)?,
                done: 0,
                cancel,
                report: &|done| {
                    progress(Progress {
                        phase: Phase::Extracting,
                        done_bytes: done,
                        total_bytes: archive_len,
                    })
                },
            };
            let mut archive =
                tar::Archive::new(bzip2::read::BzDecoder::new(io::BufReader::new(counted)));
            archive.set_preserve_permissions(false);
            archive.set_unpack_xattrs(false);
            archive
                .unpack(&staging)
                .map_err(unwrap_cancelled)
                .context("unpacking the model archive")?;

            let unpacked = staging.join(&model.archive_dir);
            if !unpacked.is_dir() {
                bail!("the archive does not contain {}", model.archive_dir);
            }
            let mut done = archive_total;
            for extra in &model.extra_files {
                let target = unpacked.join(&extra.filename);
                let base = done;
                download(&extra.url, &target, cancel, &|bytes, _| {
                    progress(Progress {
                        phase: Phase::Downloading,
                        done_bytes: base + bytes,
                        total_bytes: total,
                    });
                })
                .with_context(|| format!("downloading {}", extra.filename))?;
                done += extra.size_bytes;
            }
            fs::write(
                unpacked.join(MARKER),
                serde_json::json!({ "id": model.id, "archive": model.archive_url }).to_string(),
            )?;
            let target = self.model_dir(&model.id);
            let _ = fs::remove_dir_all(&target);
            fs::rename(&unpacked, &target)?;
            Ok(())
        })();
        let _ = fs::remove_dir_all(&staging);
        match &result {
            // A corrupt archive would fail the same way on every retry.
            Err(err) if !is_cancelled(err) && err.to_string().contains("unpacking") => {
                let _ = fs::remove_file(&part);
            }
            Ok(()) => {
                let _ = fs::remove_file(&part);
            }
            Err(_) => {}
        }
        result
    }

    /// Removes an installed model and any partial download of it.
    pub fn remove(&self, id: &str) -> Result<()> {
        if catalog::find(id).is_none() {
            bail!("unknown model {id}");
        }
        let _ = fs::remove_file(self.part_path(id));
        let dir = self.model_dir(id);
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        }
        Ok(())
    }
}

/// Whether `err` is, or was caused by, a cancellation.
pub fn is_cancelled(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| cause.is::<Cancelled>())
}

fn unwrap_cancelled(err: io::Error) -> anyhow::Error {
    if err.get_ref().is_some_and(|inner| inner.is::<Cancelled>()) {
        anyhow!(Cancelled)
    } else {
        anyhow!(err)
    }
}

struct CountingReader<'a, R> {
    inner: R,
    done: u64,
    cancel: &'a AtomicBool,
    report: &'a dyn Fn(u64),
}

impl<R: Read> Read for CountingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other(Cancelled));
        }
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        (self.report)(self.done);
        Ok(n)
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        // The operating system's TLS stack and trust store, so downloads work
        // wherever the user's browser does (corporate proxies included).
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .user_agent(concat!("speachy/", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Downloads `url` to `target`, resuming if `target` already has data.
/// `progress` receives the bytes on disk and, if known, the full length.
fn download(
    url: &str,
    target: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64, Option<u64>),
) -> Result<()> {
    const ATTEMPTS: u32 = 4;
    let agent = agent();
    let mut attempt = 0;
    loop {
        attempt += 1;
        let before = fs::metadata(target).map_or(0, |m| m.len());
        match download_once(&agent, url, target, cancel, progress) {
            Ok(()) => return Ok(()),
            Err(err) if is_cancelled(&err) => return Err(err),
            Err(err) => {
                let after = fs::metadata(target).map_or(0, |m| m.len());
                // An attempt that made progress does not count against the limit.
                if after > before {
                    attempt = 0;
                }
                if attempt >= ATTEMPTS {
                    return Err(err);
                }
                log::warn!("download interrupted ({err:#}); retrying");
                std::thread::sleep(Duration::from_millis(500 * (1 << attempt.min(4))));
            }
        }
    }
}

fn download_once(
    agent: &ureq::Agent,
    url: &str,
    target: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64, Option<u64>),
) -> Result<()> {
    let have = fs::metadata(target).map_or(0, |m| m.len());
    let mut request = agent.get(url);
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let response = request
        .call()
        .map_err(|err| anyhow!("could not reach the download server: {err}"))?;
    let status = response.status().as_u16();
    let content_length = response.body().content_length();
    let (mut file, mut done, total) = match status {
        206 => (
            OpenOptions::new().append(true).open(target)?,
            have,
            content_length.map(|len| have + len),
        ),
        200 => (File::create(target)?, 0, content_length),
        // The server has nothing past what we hold: the file is complete.
        416 if have > 0 => return Ok(()),
        _ => bail!("the download server answered {status}"),
    };
    progress(done, total);
    let mut reader = response.into_body().into_reader();
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            file.flush()?;
            return Err(anyhow!(Cancelled));
        }
        let n = reader.read(&mut buf).context("connection lost")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    file.flush()?;
    if let Some(total) = total {
        if done < total {
            bail!("connection closed early ({done} of {total} bytes)");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    /// A tiny HTTP server serving `body` for every request, with Range
    /// support. `cut_first_at` makes the first response stop early, to
    /// simulate a dropped connection.
    fn serve(body: Vec<u8>, cut_first_at: Option<usize>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut start = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(range) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        start = range.trim().trim_end_matches('-').parse().unwrap();
                    }
                }
                let slice = &body[start.min(body.len())..];
                let head = if start > 0 {
                    format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len())
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        slice.len()
                    )
                };
                let _ = stream.write_all(head.as_bytes());
                let send = match cut_first_at {
                    Some(cut) if n == 0 => &slice[..cut.min(slice.len())],
                    _ => slice,
                };
                let _ = stream.write_all(send);
            }
        });
        (url, requests)
    }

    fn archive(dir: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{dir}/{name}"), *data)
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn test_model(archive_url: String, extra_url: Option<String>) -> ModelInfo {
        let mut model = catalog::find("kitten-nano-v0.2").unwrap().clone();
        model.archive_url = archive_url;
        model.archive_dir = "kitten".into();
        model.extra_files.retain(|_| extra_url.is_some());
        if let (Some(url), Some(extra)) = (extra_url, model.extra_files.first_mut()) {
            extra.url = url;
        }
        model
    }

    #[test]
    fn installs_resumes_and_removes() {
        let payload: Vec<u8> = (0..300_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let bytes = archive(
            "kitten",
            &[("model.onnx", &payload), ("tokens.txt", b"a 1\n")],
        );
        // The first response is cut short; the retry must resume with a Range.
        let (url, requests) = serve(bytes.clone(), Some(bytes.len() / 3));
        let (extra_url, _) = serve(b"hello || h e l o\n".to_vec(), None);
        let model = test_model(url, Some(extra_url));

        let dir = tempfile::tempdir().unwrap();
        let manager = ModelManager::new(dir.path().join("models"));
        assert!(!manager.is_installed(&model.id));

        let phases = std::sync::Mutex::new(Vec::new());
        manager
            .install(&model, &AtomicBool::new(false), &|p| {
                assert!(p.done_bytes <= p.total_bytes.max(p.done_bytes));
                let mut phases = phases.lock().unwrap();
                if phases.last() != Some(&p.phase) {
                    phases.push(p.phase);
                }
            })
            .unwrap();

        assert!(
            requests.load(Ordering::SeqCst) >= 2,
            "the download was resumed"
        );
        assert!(manager.is_installed(&model.id));
        let installed = manager.model_dir(&model.id);
        assert_eq!(fs::read(installed.join("model.onnx")).unwrap(), payload);
        assert_eq!(
            fs::read(installed.join("lexicon-en-us.txt")).unwrap(),
            b"hello || h e l o\n"
        );
        assert_eq!(
            *phases.lock().unwrap(),
            [Phase::Downloading, Phase::Extracting, Phase::Downloading]
        );
        assert_eq!(
            manager.partial_bytes(&model),
            0,
            "the archive is deleted after install"
        );
        assert!(fs::read_dir(manager.root()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("staging")));

        manager.remove(&model.id).unwrap();
        assert!(!manager.is_installed(&model.id));
        assert!(manager.remove("no-such-model").is_err());
    }

    #[test]
    fn cancelling_keeps_the_partial_download() {
        let bytes = archive("kitten", &[("model.onnx", &vec![7u8; 400_000])]);
        let (url, _) = serve(bytes, None);
        let model = test_model(url, None);
        let dir = tempfile::tempdir().unwrap();
        let manager = ModelManager::new(dir.path().to_path_buf());

        let cancel = AtomicBool::new(false);
        let err = manager
            .install(&model, &cancel, &|p| {
                if p.done_bytes > 0 {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_err();
        assert!(is_cancelled(&err), "{err:#}");
        assert!(!manager.is_installed(&model.id));

        // Trying again completes the install.
        manager
            .install(&model, &AtomicBool::new(false), &|_| {})
            .unwrap();
        assert!(manager.is_installed(&model.id));
    }

    #[test]
    fn a_corrupt_archive_fails_cleanly() {
        let (url, _) = serve(b"this is not a bzip2 archive".to_vec(), None);
        let model = test_model(url, None);
        let dir = tempfile::tempdir().unwrap();
        let manager = ModelManager::new(dir.path().to_path_buf());
        assert!(manager
            .install(&model, &AtomicBool::new(false), &|_| {})
            .is_err());
        assert!(!manager.is_installed(&model.id));
        assert_eq!(
            manager.partial_bytes(&model),
            0,
            "a bad archive is not kept for resuming"
        );
    }
}
