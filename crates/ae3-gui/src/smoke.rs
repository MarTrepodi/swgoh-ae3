//! The scripted smoke run: with `AE3_GUI_SMOKE=<report.json>` set, the
//! shipped binary opens its real window, works through a short session by
//! itself, writes a JSON report and exits. CI runs it on every platform to
//! check what headless tests cannot: a window opens, OpenGL draws, the
//! network path works, and an idle window costs no CPU.
//!
//! The session: resolve the latest Asset Version and load its Manifest;
//! open the Bundle `AE3_GUI_SMOKE_BUNDLE` (default `charui_bossk`) in the
//! preview and wait for its thumbnails; open the detail viewer on its first
//! texture; let the window settle and measure the process's CPU time over
//! [`IDLE_WINDOW`]; save a screenshot of the window's own framebuffer to
//! `AE3_GUI_SMOKE_SCREENSHOT` (default: the report path with `.png`).
//!
//! The exit code is 0 when every step passed, 1 when one failed, and 2 when
//! the run took longer than `AE3_GUI_SMOKE_TIMEOUT` seconds (default 180).
//! Settings come from the usual places, but are never saved, and neither the
//! Asset Index, the Name Index nor the release check is started.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ae3_core::AssetKind;
use eframe::egui;
use serde::Serialize;

use crate::Ae3App;
use crate::model::preview::PreviewStatus;

/// The report path; setting it starts the smoke run.
pub const ENV: &str = "AE3_GUI_SMOKE";
const BUNDLE_ENV: &str = "AE3_GUI_SMOKE_BUNDLE";
const SCREENSHOT_ENV: &str = "AE3_GUI_SMOKE_SCREENSHOT";
const TIMEOUT_ENV: &str = "AE3_GUI_SMOKE_TIMEOUT";
const DEFAULT_BUNDLE: &str = "charui_bossk";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
/// How long the window is left alone before the CPU is measured.
const SETTLE: Duration = Duration::from_secs(1);
/// How long the idle CPU is measured.
pub const IDLE_WINDOW: Duration = Duration::from_secs(3);
/// How long after the timeout a window that stopped drawing is given before
/// the process exits without it.
const GRACE: Duration = Duration::from_secs(15);

/// One step of the session, as reported.
#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub name: &'static str,
    pub ok: bool,
    pub ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The JSON report.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub ok: bool,
    pub asset_version: Option<u64>,
    pub steps: Vec<Step>,
    /// Process CPU time over the idle window, as a percentage of one core;
    /// `null` when the run did not get that far.
    pub idle_cpu_percent: Option<f64>,
    /// Frames drawn over the whole run.
    pub frames: u64,
    /// Frames drawn during the idle window; one or two when the window
    /// repaints only on demand.
    pub idle_frames: Option<u64>,
    #[serde(skip)]
    timed_out: bool,
    #[serde(skip)]
    written: bool,
}

impl Report {
    fn exit_code(&self) -> i32 {
        if self.timed_out {
            2
        } else {
            i32::from(!self.ok)
        }
    }
}

type Shared = Arc<Mutex<Report>>;

fn lock(report: &Shared) -> std::sync::MutexGuard<'_, Report> {
    report.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Writes the report once; later calls do nothing.
fn write_report(report: &mut Report, path: &Path) {
    if report.written {
        return;
    }
    report.written = true;
    report.ok = !report.timed_out && !report.steps.is_empty() && report.steps.iter().all(|s| s.ok);
    let json = serde_json::to_string_pretty(&*report).unwrap_or_default();
    if let Err(e) = std::fs::write(path, json + "\n") {
        eprintln!("ae3-gui smoke: cannot write {}: {e}", path.display());
    }
}

/// Runs the smoke session and returns the process exit code.
#[must_use]
pub fn run(report_path: &Path) -> i32 {
    let bundle = std::env::var(BUNDLE_ENV).unwrap_or_else(|_| DEFAULT_BUNDLE.to_owned());
    let screenshot = std::env::var_os(SCREENSHOT_ENV)
        .map_or_else(|| report_path.with_extension("png"), PathBuf::from);
    let timeout = std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
    let deadline = Instant::now() + timeout;
    let report: Shared = Arc::default();
    let context: Arc<Mutex<Option<egui::Context>>> = Arc::default();

    // Wakes a window that waits on nothing at the deadline, and ends a
    // process whose window stopped drawing altogether.
    {
        let report = Arc::clone(&report);
        let context = Arc::clone(&context);
        let path = report_path.to_owned();
        crate::worker::spawn_long("ae3-smoke-watchdog", move || {
            std::thread::sleep(timeout);
            if let Some(ctx) = context
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            {
                ctx.request_repaint();
            }
            std::thread::sleep(GRACE);
            let mut report = lock(&report);
            report.timed_out = true;
            write_report(&mut report, &path);
            std::process::exit(2);
        });
    }

    let crate::Startup {
        config,
        file,
        notice,
        ..
    } = crate::Startup::load();
    let shared = Arc::clone(&report);
    let path = report_path.to_owned();
    let result = eframe::run_native(
        crate::APP_NAME,
        crate::native_options(),
        Box::new(move |cc| {
            *context.lock().unwrap_or_else(PoisonError::into_inner) = Some(cc.egui_ctx.clone());
            // No settings path: the run never saves the user's settings.
            let mut app = Ae3App::new(&cc.egui_ctx, config, file, None, notice);
            app.set_index_on_load(false);
            Ok(Box::new(Smoke {
                app,
                bundle,
                screenshot,
                report_path: path,
                deadline,
                report: shared,
                phase: Phase::Start,
                step_started: Instant::now(),
                shot: Arc::default(),
            }))
        }),
    );
    let mut report = lock(&report);
    if let Err(e) = result {
        report.steps.push(Step {
            name: "window",
            ok: false,
            ms: 0,
            error: Some(e.to_string()),
        });
    }
    write_report(&mut report, report_path);
    report.exit_code()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Start,
    Manifest,
    Preview,
    Thumbnails,
    Detail,
    Settle {
        until: Instant,
    },
    Idle {
        started: Instant,
        cpu: Option<Duration>,
        frames: u64,
    },
    Screenshot,
    Done,
}

/// The app with the session driving it.
struct Smoke {
    app: Ae3App,
    bundle: String,
    screenshot: PathBuf,
    report_path: PathBuf,
    deadline: Instant,
    report: Shared,
    phase: Phase,
    step_started: Instant,
    /// The screenshot's outcome, from the integration's callback.
    shot: Arc<Mutex<Option<Result<(), String>>>>,
}

impl Smoke {
    fn step_name(phase: Phase) -> &'static str {
        match phase {
            Phase::Start | Phase::Manifest => "manifest",
            Phase::Preview => "preview",
            Phase::Thumbnails => "thumbnails",
            Phase::Detail => "detail",
            Phase::Settle { .. } | Phase::Idle { .. } => "idle",
            Phase::Screenshot | Phase::Done => "screenshot",
        }
    }

    /// Ends the current step and moves to `next`.
    fn finish_step(&mut self, result: Result<(), String>, next: Phase) {
        let ms = u64::try_from(self.step_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let ok = result.is_ok();
        lock(&self.report).steps.push(Step {
            name: Self::step_name(self.phase),
            ok,
            ms,
            error: result.err(),
        });
        self.step_started = Instant::now();
        self.phase = if ok { next } else { Phase::Done };
    }

    /// Advances the session; called once per frame after the app's own
    /// work.
    fn advance(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if self.phase != Phase::Done && now >= self.deadline {
            lock(&self.report).timed_out = true;
            self.finish_step(Err("timed out".to_owned()), Phase::Done);
        }
        match self.phase {
            Phase::Start => {
                self.app.load_manifest(None);
                self.phase = Phase::Manifest;
            }
            Phase::Manifest => self.manifest(),
            Phase::Preview => self.preview(),
            Phase::Thumbnails => self.thumbnails(),
            Phase::Detail => self.detail(now),
            Phase::Settle { until } => {
                if now < until {
                    ctx.request_repaint_after(until - now);
                } else {
                    self.phase = Phase::Idle {
                        started: now,
                        cpu: process_cpu_time(),
                        frames: ctx.cumulative_frame_nr(),
                    };
                    ctx.request_repaint_after(IDLE_WINDOW);
                }
            }
            Phase::Idle {
                started,
                cpu,
                frames,
            } => {
                let until = started + IDLE_WINDOW;
                if now < until {
                    ctx.request_repaint_after(until - now);
                } else {
                    self.idle(ctx, now - started, cpu, frames);
                }
            }
            Phase::Screenshot => {
                let shot = self
                    .shot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(result) = shot {
                    self.finish_step(result, Phase::Done);
                }
            }
            Phase::Done => {}
        }
        if self.phase == Phase::Done {
            let mut report = lock(&self.report);
            report.asset_version = self.app.model.asset_version().map(|v| v.0);
            report.frames = ctx.cumulative_frame_nr();
            write_report(&mut report, &self.report_path);
            drop(report);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn manifest(&mut self) {
        if self.app.model.manifest_job.is_active() {
            return;
        }
        if self.app.store.is_none() {
            let error = self
                .app
                .last_load_error
                .clone()
                .unwrap_or_else(|| "no Manifest was loaded".to_owned());
            self.finish_step(Err(error), Phase::Done);
            return;
        }
        let row = self.app.model.rows.iter().find(|r| r.name == self.bundle);
        let Some(id) = row.map(|r| r.id) else {
            let error = format!("{} is not in the Manifest", self.bundle);
            self.finish_step(Err(error), Phase::Done);
            return;
        };
        self.finish_step(Ok(()), Phase::Preview);
        // The preview opens once the frame has drawn the new selection.
        self.app
            .model
            .click_row(id, crate::model::selection::ClickModifiers::default());
    }

    fn preview(&mut self) {
        let Some(preview) = &self.app.model.preview else {
            return;
        };
        match &preview.status {
            PreviewStatus::Loading => {}
            PreviewStatus::Bundle { .. } => {
                self.finish_step(Ok(()), Phase::Thumbnails);
            }
            PreviewStatus::Audio { .. } => {
                let error = format!("{} is an Audio Package", self.bundle);
                self.finish_step(Err(error), Phase::Done);
            }
            PreviewStatus::Failed(e) => {
                let error = e.clone();
                self.finish_step(Err(error), Phase::Done);
            }
        }
    }

    fn thumbnails(&mut self) {
        // Requests are made while the grid draws and answered in batches;
        // wait until one was answered and none is outstanding.
        let answered = !self.app.textures.is_empty() || !self.app.thumb_failed.is_empty();
        if !answered || !self.app.thumb_requested.is_empty() {
            return;
        }
        if self.app.textures.is_empty() {
            let error = self.app.thumb_failed.values().next().map_or_else(
                || "no thumbnail was decoded".to_owned(),
                |e| format!("no thumbnail was decoded: {e}"),
            );
            self.finish_step(Err(error), Phase::Done);
            return;
        }
        self.finish_step(Ok(()), Phase::Detail);
        let first = self.app.model.preview.as_ref().and_then(|p| {
            p.cells()
                .into_iter()
                .find(|a| a.kind == AssetKind::Texture)
                .cloned()
        });
        if let Some(asset) = first {
            self.app.open_detail(&asset);
        } else {
            let error = format!("{} has no texture", self.bundle);
            self.finish_step(Err(error), Phase::Done);
        }
    }

    fn detail(&mut self, now: Instant) {
        let outcome = match &self.app.detail {
            None => Some(Err("the detail viewer closed".to_owned())),
            Some(detail) => match &detail.content {
                None | Some(Ok(_)) => None,
                Some(Err(e)) => Some(Err(e.clone())),
            },
        };
        let shown = self.app.detail_texture().is_some();
        match outcome {
            Some(error) => self.finish_step(error, Phase::Done),
            None if shown => self.finish_step(
                Ok(()),
                Phase::Settle {
                    until: now + SETTLE,
                },
            ),
            None => {}
        }
    }

    fn idle(&mut self, ctx: &egui::Context, wall: Duration, cpu: Option<Duration>, frames: u64) {
        let used = cpu
            .zip(process_cpu_time())
            .map(|(a, b)| b.saturating_sub(a));
        {
            let mut report = lock(&self.report);
            report.idle_frames = Some(ctx.cumulative_frame_nr().saturating_sub(frames));
            report.idle_cpu_percent =
                used.map(|used| used.as_secs_f64() / wall.as_secs_f64() * 100.0);
        }
        // The step after the measurement, so it does not count.
        let result = used
            .map(drop)
            .ok_or_else(|| "the process CPU time cannot be read here".to_owned());
        self.finish_step(result, Phase::Screenshot);
        if self.phase == Phase::Screenshot {
            let shot = Arc::clone(&self.shot);
            let path = self.screenshot.clone();
            let wake = ctx.clone();
            ctx.request_screenshot(move |image| {
                let result = save_png(&image, &path);
                *shot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
                wake.request_repaint();
            });
        }
    }
}

/// Saves the window's pixels as a PNG.
fn save_png(image: &egui::ColorImage, path: &Path) -> Result<(), String> {
    let [width, height] = image.size;
    let width = u32::try_from(width).map_err(|e| e.to_string())?;
    let height = u32::try_from(height).map_err(|e| e.to_string())?;
    let rgba = ae3_core::RgbaImage::new(width, height, image.as_raw().to_vec())
        .map_err(|e| e.to_string())?;
    let png = ae3_core::ae3_image::encode(&rgba, ae3_core::ae3_image::ImageFormat::default())
        .map_err(|e| e.to_string())?;
    std::fs::write(path, png).map_err(|e| format!("{}: {e}", path.display()))
}

impl eframe::App for Smoke {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
        self.advance(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.app.on_exit(gl);
    }
}

/// CPU time this process has used, all threads together.
#[cfg(target_os = "linux")]
fn process_cpu_time() -> Option<Duration> {
    // Fields 14 and 15 of /proc/self/stat, after the command in brackets:
    // user and system time in clock ticks, which the kernel reports to user
    // space at USER_HZ = 100.
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace().skip(11);
    let user: u64 = fields.next()?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    Some(Duration::from_millis((user + system) * 10))
}

/// CPU time this process has used, all threads together, as `ps` reports
/// it: `[[dd-]hh:]mm:ss.cc`.
#[cfg(all(unix, not(target_os = "linux")))]
fn process_cpu_time() -> Option<Duration> {
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    let (days, clock) = text.split_once('-').unwrap_or(("0", text));
    let mut seconds = days.parse::<f64>().ok()? * 86_400.0;
    let mut scale = 1.0;
    for part in clock.rsplit(':') {
        seconds += part.parse::<f64>().ok()? * scale;
        scale *= 60.0;
    }
    Duration::try_from_secs_f64(seconds).ok()
}

/// CPU time this process has used, all threads together, as PowerShell
/// reports it.
#[cfg(windows)]
fn process_cpu_time() -> Option<Duration> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = format!(
        "(Get-Process -Id {}).TotalProcessorTime.TotalMilliseconds",
        std::process::id()
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let ms: f64 = String::from_utf8(out.stdout).ok()?.trim().parse().ok()?;
    Duration::try_from_secs_f64(ms / 1000.0).ok()
}

#[cfg(not(any(unix, windows)))]
fn process_cpu_time() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_is_readable_and_grows() {
        let before = process_cpu_time().expect("CPU time on this platform");
        let start = Instant::now();
        let mut x = 0_u64;
        while start.elapsed() < Duration::from_millis(300) {
            x = x.wrapping_mul(31).wrapping_add(7);
        }
        std::hint::black_box(x);
        let after = process_cpu_time().unwrap();
        assert!(after > before, "{before:?} -> {after:?}");
    }

    #[test]
    fn report_shape_matches_the_contract() {
        let mut report = Report {
            asset_version: Some(100_054),
            steps: vec![
                Step {
                    name: "manifest",
                    ok: true,
                    ms: 12,
                    error: None,
                },
                Step {
                    name: "preview",
                    ok: false,
                    ms: 3,
                    error: Some("boom".into()),
                },
            ],
            idle_cpu_percent: Some(0.5),
            frames: 40,
            ..Report::default()
        };
        let dir = std::env::temp_dir().join(format!("ae3-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report.json");
        write_report(&mut report, &path);
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["asset_version"], 100_054);
        assert_eq!(value["steps"][0]["name"], "manifest");
        assert!(value["steps"][0].get("error").is_none());
        assert_eq!(value["steps"][1]["error"], "boom");
        assert_eq!(value["frames"], 40);
        assert_eq!(report.exit_code(), 1);
        report.timed_out = true;
        assert_eq!(report.exit_code(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
