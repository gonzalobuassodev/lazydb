use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use unicode_width::UnicodeWidthChar;

use super::{UpdateProgress, UpdateReport, UpdateStage, channel_name};

const REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

pub(super) struct CliProgress {
    state: Arc<Mutex<ProgressOutput<io::Stdout>>>,
    stop: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl CliProgress {
    pub(super) fn start() -> io::Result<Self> {
        let stdout = io::stdout();
        let interactive =
            stdout.is_terminal() && !std::env::var("TERM").is_ok_and(|term| term == "dumb");
        let mut output = ProgressOutput::new(stdout, interactive);
        output.line(&format!(
            "LazyDB update\n  Current      {}",
            env!("CARGO_PKG_VERSION")
        ))?;
        output.wait("Checking for updates...")?;
        let state = Arc::new(Mutex::new(output));
        let (stop, worker) = if interactive {
            let (sender, receiver) = mpsc::channel();
            let worker_state = Arc::clone(&state);
            let worker = thread::spawn(move || {
                while matches!(
                    receiver.recv_timeout(REFRESH_INTERVAL),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    let mut output = worker_state.lock().unwrap();
                    let result = output.render();
                    output.remember_error(result);
                }
            });
            (Some(sender), Some(worker))
        } else {
            (None, None)
        };
        Ok(Self {
            state,
            stop,
            worker,
        })
    }

    pub(super) fn available(&self, report: &UpdateReport) -> io::Result<()> {
        let mut output = self.state.lock().unwrap();
        output.end_line()?;
        output.line(&format!(
            "[INFO] Update available: {} -> {} ({})",
            report.current_version.as_deref().unwrap_or("unknown"),
            report.target_version.as_deref().unwrap_or("unknown"),
            channel_name(report.channel)
        ))?;
        output.wait("Preparing update...")
    }

    pub(super) fn progress(&self, progress: UpdateProgress) {
        let mut output = self.state.lock().unwrap();
        let result = output.progress(progress);
        output.remember_error(result);
    }

    pub(super) fn finish(&mut self) -> io::Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let mut output = self.state.lock().unwrap();
        output.end_line()?;
        if let Some(error) = output.error.take() {
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for CliProgress {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

struct ProgressOutput<W> {
    writer: W,
    interactive: bool,
    message: Option<String>,
    stage: Option<UpdateStage>,
    download: Option<UpdateProgress>,
    frame: usize,
    error: Option<io::Error>,
}

impl<W: Write> ProgressOutput<W> {
    fn new(writer: W, interactive: bool) -> Self {
        Self {
            writer,
            interactive,
            message: None,
            stage: None,
            download: None,
            frame: 0,
            error: None,
        }
    }

    fn remember_error(&mut self, result: io::Result<()>) {
        if self.error.is_none() {
            self.error = result.err();
        }
    }

    fn line(&mut self, message: &str) -> io::Result<()> {
        writeln!(self.writer, "{message}")?;
        self.writer.flush()
    }

    fn wait(&mut self, message: &str) -> io::Result<()> {
        self.message = Some(message.to_owned());
        if self.interactive {
            self.render()
        } else {
            self.line(&format!("[INFO] {message}"))
        }
    }

    fn progress(&mut self, progress: UpdateProgress) -> io::Result<()> {
        if self.stage != Some(progress.stage) {
            self.end_line()?;
            self.stage = Some(progress.stage);
            // The next stage carries the final byte count even when HTTP has no total.
            if progress.stage != UpdateStage::Downloading {
                self.download = None;
            }
            self.wait(stage_label(progress.stage))?;
        }
        if progress.stage == UpdateStage::Downloading {
            self.download = Some(progress);
        }
        // The heartbeat renders the most recent bytes at a fixed rate. Plain logs
        // retain stages and the final download size without logging every chunk.
        Ok(())
    }

    fn render(&mut self) -> io::Result<()> {
        let Some(message) = &self.message else {
            return Ok(());
        };
        let width = terminal_width();
        let message = self
            .download
            .as_ref()
            .map(|progress| download_label_for_width(progress, width.saturating_sub(4)))
            .unwrap_or_else(|| message.clone());
        let line = format!("  {} {message}", SPINNER[self.frame % SPINNER.len()]);
        self.frame = self.frame.wrapping_add(1);
        write!(self.writer, "\r\x1b[2K{}", fit_line(&line, width))?;
        self.writer.flush()
    }

    fn end_line(&mut self) -> io::Result<()> {
        if let Some(message) = self.message.take() {
            let width = if self.interactive {
                terminal_width()
            } else {
                usize::MAX
            };
            let message = self
                .download
                .take()
                .as_ref()
                .map(|progress| download_label_for_width(progress, width.saturating_sub(2)))
                .unwrap_or(message);
            if self.interactive {
                write!(self.writer, "\r\x1b[2K")?;
                self.line(&fit_line(&format!("  {message}"), width))?;
            } else if self.stage == Some(UpdateStage::Downloading) {
                self.line(&format!("[INFO] {message}"))?;
            }
        }
        Ok(())
    }
}

fn stage_label(stage: UpdateStage) -> &'static str {
    match stage {
        UpdateStage::Preparing => "Preparing download...",
        UpdateStage::Downloading => "Downloading update...",
        UpdateStage::VerifyingChecksum => "Verifying checksum...",
        UpdateStage::Extracting => "Extracting update...",
        UpdateStage::VerifyingInstallation => "Verifying installation...",
        UpdateStage::Activating => "Activating update...",
        UpdateStage::Finishing => "Finishing update...",
    }
}

fn download_label(progress: &UpdateProgress) -> String {
    let downloaded = format_bytes(progress.downloaded_bytes);
    match progress.total_bytes.filter(|total| *total > 0) {
        Some(total) => {
            let ratio = (progress.downloaded_bytes as f64 / total as f64).clamp(0.0, 1.0);
            let filled = (ratio * 20.0) as usize;
            // Only a fully received download may show 100%.
            let percent = (ratio * 100.0).floor() as u8;
            format!(
                "Downloading [{}{}] {percent:>3}%  {downloaded} / {}",
                "=".repeat(filled),
                "-".repeat(20 - filled),
                format_bytes(total)
            )
        }
        None => format!("Downloading {downloaded} (total size unknown)"),
    }
}

fn download_label_for_width(progress: &UpdateProgress, width: usize) -> String {
    let full = download_label(progress);
    if full.len() <= width {
        return full;
    }
    let downloaded = format_bytes(progress.downloaded_bytes);
    let Some(total) = progress.total_bytes.filter(|total| *total > 0) else {
        let compact = format!("Download {downloaded} (size unknown)");
        return if compact.len() <= width {
            compact
        } else {
            format!("{downloaded} received")
        };
    };
    let ratio = (progress.downloaded_bytes as f64 / total as f64).clamp(0.0, 1.0);
    let percent = (ratio * 100.0).floor() as u8;
    let compact = format!("Download {percent}% {downloaded}/{}", format_bytes(total));
    let bar_width = width.saturating_sub(compact.len() + 3).min(20);
    if bar_width >= 5 {
        let filled = (ratio * bar_width as f64) as usize;
        format!(
            "{compact} [{}{}]",
            "=".repeat(filled),
            "-".repeat(bar_width - filled)
        )
    } else if compact.len() <= width {
        compact
    } else {
        format!("{percent}% {downloaded} received")
    }
}

fn terminal_width() -> usize {
    crossterm::terminal::size()
        .map(|(width, _)| usize::from(width).saturating_sub(1))
        .unwrap_or(79)
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn fit_line(line: &str, width: usize) -> String {
    let mut used = 0;
    line.chars()
        .take_while(|character| {
            used += character.width().unwrap_or(0);
            used <= width
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_percentage_is_based_on_bytes_and_requires_a_nonzero_total() {
        let mut progress = UpdateProgress {
            stage: UpdateStage::Downloading,
            downloaded_bytes: 512,
            total_bytes: Some(1024),
        };
        assert!(download_label(&progress).contains("50%"));
        progress.downloaded_bytes = 1023;
        assert!(download_label(&progress).contains("99%"));
        progress.downloaded_bytes = 2048;
        assert!(download_label(&progress).contains("100%"));
        for total in [None, Some(0)] {
            progress.total_bytes = total;
            let label = download_label(&progress);
            assert!(label.contains("2.0 KiB"), "{label}");
            assert!(!label.contains('%'), "{label}");
        }
    }

    #[test]
    fn narrow_downloads_prioritize_percentage_and_received_bytes() {
        let mut progress = UpdateProgress {
            stage: UpdateStage::Downloading,
            downloaded_bytes: 8 * 1024 * 1024,
            total_bytes: Some(16 * 1024 * 1024),
        };
        for width in [35, 25] {
            let label = download_label_for_width(&progress, width);
            assert!(label.len() <= width, "{label}");
            assert!(
                label.contains("50%") && label.contains("8.0 MiB"),
                "{label}"
            );
        }
        progress.total_bytes = None;
        let label = download_label_for_width(&progress, 25);
        assert!(label.len() <= 25 && label.contains("8.0 MiB"), "{label}");
        assert!(!label.contains('%'), "{label}");
    }

    #[test]
    fn plain_logs_retain_stages_and_final_bytes_without_chunk_noise() {
        let mut output = ProgressOutput::new(Vec::new(), false);
        output.wait("Checking for updates...").unwrap();
        for downloaded_bytes in [0, 256, 512, 1024] {
            output
                .progress(UpdateProgress {
                    stage: UpdateStage::Downloading,
                    downloaded_bytes,
                    total_bytes: Some(1024),
                })
                .unwrap();
        }
        for stage in [UpdateStage::VerifyingChecksum, UpdateStage::Activating] {
            output
                .progress(UpdateProgress {
                    stage,
                    downloaded_bytes: 1024,
                    total_bytes: Some(1024),
                })
                .unwrap();
        }
        output.end_line().unwrap();
        let text = String::from_utf8(output.writer).unwrap();
        assert!(!text.contains('\x1b') && !text.contains('\r'), "{text}");
        assert_eq!(text.matches("Downloading").count(), 2, "{text}");
        assert!(text.contains("100%  1.0 KiB / 1.0 KiB"), "{text}");
        assert!(text.find("100%").unwrap() < text.find("Verifying checksum").unwrap());
        assert!(text.find("Verifying checksum").unwrap() < text.find("Activating").unwrap());
        assert!(!text.contains("success"), "{text}");
    }

    #[test]
    fn ending_interactive_output_clears_the_active_line() {
        let mut output = ProgressOutput::new(Vec::new(), true);
        output.wait("Checking for updates...").unwrap();
        output.end_line().unwrap();
        let size = output.writer.len();
        output.render().unwrap();
        assert_eq!(output.writer.len(), size);
        let text = String::from_utf8(output.writer).unwrap();
        assert!(text.contains("\r\x1b[2K"), "{text}");
        assert!(text.ends_with("Checking for updates...\n"), "{text}");
        assert_eq!(fit_line("  Downloading 更新", 17), "  Downloading 更");
    }
}
