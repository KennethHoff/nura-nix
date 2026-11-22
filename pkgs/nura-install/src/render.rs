//! Pure text for the terminal: durations, finished-step lines, the spinner line and counts.
//! No clock, no I/O: every function maps values to bytes so the tests can compare them exactly.

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const WIDTH: usize = 64;
const CLEAR_LINE: &str = "\r\x1b[K";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Pass,
    Fail,
}

impl Mark {
    pub fn tag(self) -> &'static str {
        match self {
            Mark::Pass => "[ ok ]",
            Mark::Fail => "[FAIL]",
        }
    }
}

/// `x.y s` under ten seconds, whole seconds up to a minute, then `m:ss`, and `h:mm:ss` from an hour.
pub fn duration(ms: u64) -> String {
    if ms < 10_000 {
        return format!("{}.{} s", ms / 1000, (ms % 1000) / 100);
    }
    let secs = (ms + 500) / 1000;
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{}:{:02}", secs / 60, secs % 60),
        _ => format!("{}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60),
    }
}

/// `m:ss`, or `h:mm:ss` from an hour on: the clocks of the terminal title.
pub fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// What the terminal title says.
pub enum Title<'a> {
    /// The tool works.
    Working {
        n: u32,
        total: u32,
        label: &'a str,
        step_ms: u64,
        run_ms: u64,
    },
    Done {
        run_ms: u64,
    },
    Stopped {
        n: u32,
        total: u32,
        run_ms: u64,
    },
}

/// The title text; `tick` picks the spinner frame and advances once per update.
pub fn title(state: &Title, tick: usize) -> String {
    let frame = FRAMES[tick % FRAMES.len()];
    match *state {
        Title::Working {
            n,
            total,
            label,
            step_ms,
            run_ms,
        } => {
            format!(
                "{frame} nura {n}/{total} {label} · {} ({})",
                clock(step_ms),
                clock(run_ms)
            )
        }
        Title::Done { run_ms } => format!("✓ nura done · {}", clock(run_ms)),
        Title::Stopped { n, total, run_ms } => {
            format!("✗ nura stopped at {n}/{total} · {}", clock(run_ms))
        }
    }
}

/// The escape sequence that sets the terminal title (OSC 2).
pub fn set_title(text: &str) -> String {
    format!("\x1b]2;{text}\x07")
}

pub fn human(bytes: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)];
    UNITS.iter().find(|(_, size)| bytes >= *size).map_or_else(
        || format!("{bytes} bytes"),
        |(unit, size)| format!("{:.1} {unit}", bytes as f64 / *size as f64),
    )
}

/// `1 check`, `2 checks`, `0 checks`.
pub fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A line that ends a step: two-space indent, `text` padded to a fixed column, then the duration.
pub fn finished(text: &str, ms: u64) -> String {
    format!("  {text:<WIDTH$} {}", duration(ms))
}

/// A check's result: the mark, the label and the first line of `detail` carry the duration; the
/// rest of `detail` hangs below, aligned under the label.
pub fn result(mark: Mark, label: &str, detail: &str, ms: u64) -> String {
    let mut lines = detail.lines();
    let head = match lines.next() {
        Some(first) => format!("{} {label}: {first}", mark.tag()),
        None => format!("{} {label}", mark.tag()),
    };
    let mut out = finished(&head, ms);
    for line in lines {
        out.push_str(&format!("\n         {line}"));
    }
    out
}

pub fn run_end(ms: u64, outcome: &str) -> String {
    format!("done in {}: {outcome}", duration(ms))
}

/// The in-place line of a running step: clears the line, then frame, label and elapsed time.
/// A line wider than `cols` loses the end of its label, since a wrapped line cannot be redrawn.
pub fn spinner(tick: usize, label: &str, elapsed_ms: u64, cols: usize) -> String {
    let tail = format!("  {}", duration(elapsed_ms));
    let frame = FRAMES[tick % FRAMES.len()];
    let room = cols.saturating_sub(4 + tail.chars().count() + 1);
    let label = if label.chars().count() > room {
        let kept: String = label.chars().take(room.saturating_sub(1)).collect();
        format!("{kept}…")
    } else {
        label.to_string()
    };
    format!("{CLEAR_LINE}  {frame} {label}{tail}")
}

pub fn clear() -> &'static str {
    CLEAR_LINE
}

/// The plain line the transcript gets now and then for a long step.
pub fn progress_line(label: &str, elapsed_ms: u64) -> String {
    format!("  ... {label}: still running, {}", duration(elapsed_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_under_ten_seconds_show_a_tenth() {
        assert_eq!(duration(0), "0.0 s");
        assert_eq!(duration(99), "0.0 s");
        assert_eq!(duration(1234), "1.2 s");
        assert_eq!(duration(9999), "9.9 s");
    }

    #[test]
    fn durations_from_ten_seconds_are_whole_seconds_until_a_minute() {
        assert_eq!(duration(10_000), "10 s");
        assert_eq!(duration(10_499), "10 s");
        assert_eq!(duration(10_500), "11 s");
        assert_eq!(duration(59_499), "59 s");
    }

    #[test]
    fn durations_from_a_minute_are_minutes_and_seconds() {
        assert_eq!(duration(59_500), "1:00");
        assert_eq!(duration(65_000), "1:05");
        assert_eq!(duration(754_000), "12:34");
        assert_eq!(duration(3_599_499), "59:59");
    }

    #[test]
    fn durations_from_an_hour_carry_hours() {
        assert_eq!(duration(3_599_500), "1:00:00");
        assert_eq!(duration(3_725_000), "1:02:05");
    }

    #[test]
    fn the_title_says_step_work_and_two_clocks() {
        let working = Title::Working {
            n: 2,
            total: 14,
            label: "Check the phone over adb",
            step_ms: 12_000,
            run_ms: 41_000,
        };
        assert_eq!(
            title(&working, 1),
            "⠙ nura 2/14 Check the phone over adb · 0:12 (0:41)"
        );
        assert_eq!(
            title(&Title::Done { run_ms: 2_480_000 }, 0),
            "✓ nura done · 41:20"
        );
        assert_eq!(
            title(
                &Title::Stopped {
                    n: 7,
                    total: 14,
                    run_ms: 1_082_000
                },
                0
            ),
            "✗ nura stopped at 7/14 · 18:02"
        );
        assert_eq!(clock(3_725_000), "1:02:05");
        assert_eq!(set_title("x"), "\x1b]2;x\x07");
        assert_eq!(title(&working, 11), title(&working, 1));
    }

    #[test]
    fn sizes_use_the_largest_whole_unit() {
        assert_eq!(human(0), "0 bytes");
        assert_eq!(human(1023), "1023 bytes");
        assert_eq!(human(1024), "1.0 KiB");
        assert_eq!(human(789 << 20), "789.0 MiB");
        assert_eq!(human(17 << 30), "17.0 GiB");
    }

    #[test]
    fn counts_are_singular_only_for_one() {
        assert_eq!(plural(0, "check"), "0 checks");
        assert_eq!(plural(1, "check"), "1 check");
        assert_eq!(plural(2, "check"), "2 checks");
    }

    #[test]
    fn a_finished_line_pads_its_text_to_the_duration_column() {
        let line = finished("[ ok ] serial: ABC", 1234);
        assert_eq!(line, format!("  {:<64} 1.2 s", "[ ok ] serial: ABC"));
        assert_eq!(line.chars().count(), 2 + 64 + 1 + 5);
    }

    #[test]
    fn text_wider_than_the_column_keeps_one_space_before_the_duration() {
        let wide = "x".repeat(70);
        assert_eq!(finished(&wide, 12_000), format!("  {wide} 12 s"));
    }

    #[test]
    fn a_result_hangs_extra_detail_lines_below_without_a_duration() {
        let text = result(Mark::Fail, "hash list", "2 files\n- one\n- two", 65_000);
        assert_eq!(
            text,
            format!(
                "  {:<64} 1:05\n         - one\n         - two",
                "[FAIL] hash list: 2 files"
            )
        );
        assert_eq!(
            result(Mark::Pass, "step", "", 0),
            format!("  {:<64} 0.0 s", "[ ok ] step")
        );
    }

    #[test]
    fn the_run_ends_with_its_total_and_outcome() {
        assert_eq!(run_end(42_000, "finished"), "done in 42 s: finished");
        assert_eq!(run_end(83_000, "stopped"), "done in 1:23: stopped");
    }

    #[test]
    fn the_spinner_rewrites_its_line_with_a_braille_frame() {
        assert_eq!(
            spinner(0, "hashing", 1234, 80),
            "\r\x1b[K  ⠋ hashing  1.2 s"
        );
        assert_eq!(
            spinner(11, "hashing", 65_000, 80),
            "\r\x1b[K  ⠙ hashing  1:05"
        );
        let frames: String = (0..10)
            .map(|t| {
                spinner(t, "x", 0, 80)
                    .trim_start_matches("\r\x1b[K  ")
                    .chars()
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(frames, "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");
    }

    #[test]
    fn a_long_label_is_cut_so_the_line_never_wraps() {
        let label = "a".repeat(100);
        let line = spinner(0, &label, 1000, 40);
        let shown = line.trim_start_matches("\r\x1b[K");
        assert!(shown.chars().count() < 40, "{shown}");
        assert!(shown.ends_with("…  1.0 s"), "{shown}");
    }

    #[test]
    fn clearing_the_line_is_carriage_return_and_erase() {
        assert_eq!(clear(), "\r\x1b[K");
    }

    #[test]
    fn the_transcript_progress_line_is_plain_text() {
        assert_eq!(
            progress_line("waiting", 30_000),
            "  ... waiting: still running, 30 s"
        );
        assert!(!progress_line("x", 1).contains('\x1b'));
    }

    #[test]
    fn marks_keep_their_tags() {
        assert_eq!(Mark::Pass.tag(), "[ ok ]");
        assert_eq!(Mark::Fail.tag(), "[FAIL]");
    }
}
