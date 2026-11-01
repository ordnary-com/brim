//! Terminal presentation: colors, symbols, spinners and timings.
//!
//! The rich output is only for people at a terminal. With
//! [`ColorChoice::Never`] (pipes, `NO_COLOR`, tests, embedders) every command
//! prints the same plain lines as before, so scripts and snapshot tests keep
//! working; callers pass both forms and [`Ui`] picks one.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use brim_diagnostics::ColorChoice;

use crate::Context;

/// Brim blue (#4d84ff, the readable tint of the logo's #015efe).
const BRAND: &str = "38;2;77;132;255";
const GREEN: &str = "32";
const RED: &str = "31";
const DIM: &str = "2";
const BOLD: &str = "1";

/// How output is styled for a context.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ui {
    pub(crate) on: bool,
}

impl Ui {
    pub(crate) fn new(ctx: &Context<'_>) -> Self {
        Ui {
            on: ctx.color == ColorChoice::Always,
        }
    }

    /// For text on standard output, which can be piped while standard error
    /// is still a terminal.
    pub(crate) fn stdout(ctx: &Context<'_>) -> Self {
        Ui {
            on: ctx.color == ColorChoice::Always && io::stdout().is_terminal(),
        }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.on && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub(crate) fn brand(self, text: &str) -> String {
        self.paint(BRAND, text)
    }
    pub(crate) fn bold(self, text: &str) -> String {
        self.paint(BOLD, text)
    }
    pub(crate) fn dim(self, text: &str) -> String {
        self.paint(DIM, text)
    }
    pub(crate) fn green(self, text: &str) -> String {
        self.paint(GREEN, text)
    }
    pub(crate) fn red(self, text: &str) -> String {
        self.paint(RED, text)
    }

    /// A badge: bold text on a colored background, padded.
    pub(crate) fn badge(self, background: Badge, text: &str) -> String {
        let code = match background {
            Badge::Brand => "1;97;48;2;1;94;254",
            Badge::Pass => "1;30;42",
            Badge::Fail => "1;97;41",
        };
        self.paint(code, &format!(" {text} "))
    }

    /// `✓ Verb detail  12ms`, the line that ends a successful step.
    pub(crate) fn success(self, verb: &str, detail: &str, elapsed: Option<Duration>) -> String {
        self.line(&self.green("✓"), verb, detail, elapsed)
    }

    /// `• Verb detail`, for a step that needed no work.
    pub(crate) fn skipped(self, verb: &str, detail: &str) -> String {
        self.line(&self.dim("•"), verb, detail, None)
    }

    /// `+ Verb detail`, for one item of a larger step.
    pub(crate) fn item(self, verb: &str, detail: &str) -> String {
        self.line(&self.brand("+"), verb, detail, None)
    }

    fn line(self, symbol: &str, verb: &str, detail: &str, elapsed: Option<Duration>) -> String {
        let mut line = format!("{symbol} {}", self.bold(verb));
        if !detail.is_empty() {
            line.push(' ');
            line.push_str(detail);
        }
        if let Some(elapsed) = elapsed {
            line.push_str("  ");
            line.push_str(&self.dim(&duration(elapsed)));
        }
        line
    }

    /// Colors the main help: headings bold, commands and options in Brim
    /// blue, the first line as a title.
    pub(crate) fn help(self, text: &str) -> String {
        if !self.on {
            return text.to_string();
        }
        let mut out = String::new();
        for (i, line) in text.lines().enumerate() {
            if i == 0 {
                out.push_str(&format!(
                    "{} {}\n",
                    self.badge(Badge::Brand, "brim"),
                    self.bold(line.strip_prefix("Brim: ").unwrap_or(line))
                ));
            } else if let Some(rest) = line.strip_prefix("Usage: ") {
                out.push_str(&format!("{} {}\n", self.bold("Usage:"), self.brand(rest)));
            } else if line.ends_with(':') && !line.starts_with(' ') {
                out.push_str(&self.bold(line));
                out.push('\n');
            } else if let Some(entry) = line.strip_prefix("  ") {
                // "  name args   Description": the name column ends at the
                // first run of two spaces.
                match entry.find("  ") {
                    Some(at) => {
                        let (name, description) = entry.split_at(at);
                        out.push_str(&format!("  {}{description}\n", self.brand(name)));
                    }
                    None => {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            } else {
                out.push_str(&self.dim(line));
                out.push('\n');
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Badge {
    Brand,
    Pass,
    Fail,
}

/// `84ms`, `1.24s`, `2m 03s`.
pub(crate) fn duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.2}s", d.as_secs_f64())
    } else {
        format!("{}m {:02}s", ms / 60_000, (ms / 1000) % 60)
    }
}

/// Ends a step: `plain` without colors, else a `✓ Verb detail` line.
pub(crate) fn done(
    ctx: &mut Context<'_>,
    plain: &str,
    verb: &str,
    detail: &str,
    elapsed: Option<Duration>,
) {
    let ui = Ui::new(ctx);
    let line = if ui.on {
        ui.success(verb, detail, elapsed)
    } else {
        plain.to_string()
    };
    let _ = writeln!(ctx.stderr, "{line}");
}

// ---- spinner ----------------------------------------------------------

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME_TIME: Duration = Duration::from_millis(80);
/// Quick steps finish before the spinner shows, instead of flickering.
const SHOW_AFTER: Duration = Duration::from_millis(120);

struct Running {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// The one spinner on screen. Global so that anything about to print
/// (diagnostics above all) can take it down first with [`stop`].
static SPINNER: Mutex<Option<Running>> = Mutex::new(None);

/// Shows `message` with a spinner and the time so far on standard error,
/// until the returned guard is dropped or [`stop`] is called. Does nothing
/// unless standard error is a terminal with colors on.
pub(crate) fn spin(ctx: &Context<'_>, message: &str) -> Spinner {
    if !(Ui::new(ctx).on && io::stderr().is_terminal()) {
        return Spinner;
    }
    stop();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop_flag);
    // Leading spaces indent the spinner too, so it lines up with the
    // marks that replace it.
    let indent = message.len() - message.trim_start().len();
    let (indent, message) = (message[..indent].to_string(), message[indent..].to_string());
    let ui = Ui { on: true };
    let spawned = std::thread::Builder::new()
        .name("brim-spinner".into())
        .spawn(move || {
            let start = Instant::now();
            let mut shown = false;
            let mut frame = 0;
            while !flag.load(Ordering::Acquire) {
                let elapsed = start.elapsed();
                if elapsed >= SHOW_AFTER {
                    let mut err = io::stderr().lock();
                    // Hide the cursor while spinning; `\x1b[2K` clears the line.
                    let _ = write!(
                        err,
                        "{}\r\x1b[2K{indent}{} {} {}",
                        if shown { "" } else { "\x1b[?25l" },
                        ui.brand(FRAMES[frame % FRAMES.len()]),
                        message,
                        ui.dim(&duration(elapsed))
                    );
                    let _ = err.flush();
                    shown = true;
                    frame += 1;
                }
                std::thread::park_timeout(FRAME_TIME);
            }
            if shown {
                let mut err = io::stderr().lock();
                let _ = write!(err, "\r\x1b[2K\x1b[?25h");
                let _ = err.flush();
            }
        });
    if let Ok(thread) = spawned
        && let Ok(mut slot) = SPINNER.lock()
    {
        *slot = Some(Running {
            stop: stop_flag,
            thread,
        });
    }
    Spinner
}

/// Takes the spinner down and clears its line, if one is showing.
pub(crate) fn stop() {
    let running = SPINNER.lock().ok().and_then(|mut slot| slot.take());
    if let Some(Running { stop, thread }) = running {
        stop.store(true, Ordering::Release);
        thread.thread().unpark();
        let _ = thread.join();
    }
}

/// Ends the spinner when dropped.
#[derive(Debug)]
pub(crate) struct Spinner;

impl Drop for Spinner {
    fn drop(&mut self) {
        stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration(Duration::from_millis(84)), "84ms");
        assert_eq!(duration(Duration::from_millis(1240)), "1.24s");
        assert_eq!(duration(Duration::from_secs(123)), "2m 03s");
    }

    #[test]
    fn plain_when_off() {
        let ui = Ui { on: false };
        assert_eq!(ui.success("Checked", "a.brim", None), "✓ Checked a.brim");
        assert_eq!(
            ui.help("Brim: x\n\nUsage: brim\n"),
            "Brim: x\n\nUsage: brim\n"
        );
    }

    #[test]
    fn help_columns() {
        let ui = Ui { on: true };
        let out = ui.help("Commands:\n  check [PATH]   Check it\n");
        assert!(out.contains("\x1b[38;2;77;132;255mcheck [PATH]\x1b[0m   Check it"));
    }
}
