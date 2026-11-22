//! Pure text for the terminal: durations, finished-step lines, the spinner line, bars and counts.
//! No clock, no I/O: every function maps values to bytes so the tests can compare them exactly.

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const WIDTH: usize = 64;
const BAR_CELLS: u128 = 24;
const CLEAR_LINE: &str = "\r\x1b[K";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Pass,
    Fail,
    Skip,
}

impl Mark {
    pub fn tag(self) -> &'static str {
        match self {
            Mark::Pass => "[ ok ]",
            Mark::Fail => "[FAIL]",
            Mark::Skip => "[skip]",
        }
    }
}

/// What a running step has measured: a real count, with its total only when that is truly known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Meter {
    None,
    Bytes { done: u64, total: Option<u64> },
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
    /// The tool works; `pct` only where the screen shows a bar.
    Working {
        n: u32,
        total: u32,
        label: &'a str,
        pct: Option<u64>,
        step_ms: u64,
        run_ms: u64,
    },
    /// The run waits for the owner: an action, a typed answer or the phone.
    Waiting {
        n: u32,
        total: u32,
        what: &'a str,
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
            pct,
            step_ms,
            run_ms,
        } => {
            let pct = pct.map_or(String::new(), |p| format!(" {p}%"));
            format!(
                "{frame} nura {n}/{total} {label}{pct} · {} ({})",
                clock(step_ms),
                clock(run_ms)
            )
        }
        Title::Waiting {
            n,
            total,
            what,
            step_ms,
            run_ms,
        } => format!(
            "⏸ nura {n}/{total} {what} · {} ({})",
            clock(step_ms),
            clock(run_ms)
        ),
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

/// A limit named before a wait: `45 s`, `15 min`, `1 min 30 s`.
/// ` (up to …)` after a wait's name when it has a limit, nothing when it waits until stopped.
pub fn up_to(limit_of: Option<std::time::Duration>) -> String {
    limit_of.map_or(String::new(), |d| {
        format!(" (up to {})", limit(d.as_secs()))
    })
}

pub fn limit(secs: u64) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
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

/// A result that never ran, so it has no duration.
pub fn skipped(label: &str, why: &str) -> String {
    format!("  {} {label}: {why}", Mark::Skip.tag())
}

pub fn run_end(ms: u64, outcome: &str) -> String {
    format!("done in {}: {outcome}", duration(ms))
}

pub fn percent(done: u64, total: u64) -> u64 {
    if total == 0 {
        return 100;
    }
    (u128::from(done.min(total)) * 100 / u128::from(total)) as u64
}

/// 24 cells, filled in proportion to `done` of `total`.
pub fn bar(done: u64, total: u64) -> String {
    let filled = if total == 0 {
        BAR_CELLS
    } else {
        u128::from(done.min(total)) * BAR_CELLS / u128::from(total)
    };
    (0..BAR_CELLS)
        .map(|i| if i < filled { '█' } else { '░' })
        .collect()
}

fn rate(done: u64, elapsed_ms: u64) -> Option<String> {
    (done > 0 && elapsed_ms > 0).then(|| {
        format!(
            "{}/s",
            human((u128::from(done) * 1000 / u128::from(elapsed_ms)) as u64)
        )
    })
}

/// What the meter says. With `with_bar` a known total gets the bar, as on the screen; without it the
/// same facts are words, for the transcript.
pub fn meter_text(meter: Meter, elapsed_ms: u64, with_bar: bool) -> String {
    match meter {
        Meter::None => String::new(),
        Meter::Bytes {
            done,
            total: Some(total),
        } if done <= total => {
            let pct = percent(done, total);
            let mut text = if with_bar {
                format!(
                    "{}  {} / {}  {pct}%",
                    bar(done, total),
                    human(done),
                    human(total)
                )
            } else {
                format!("{} of {} ({pct}%)", human(done), human(total))
            };
            if let Some(r) = rate(done, elapsed_ms) {
                text.push_str(if with_bar { "  " } else { ", " });
                text.push_str(&r);
            }
            text
        }
        Meter::Bytes { done, .. } => match rate(done, elapsed_ms) {
            Some(r) => format!("{}, {r}", human(done)),
            None => human(done),
        },
    }
}

/// The in-place line of a running step: clears the line, then frame, label, meter and elapsed time.
/// A line wider than `cols` loses the end of its label, since a wrapped line cannot be redrawn.
pub fn spinner(tick: usize, label: &str, meter: &str, elapsed_ms: u64, cols: usize) -> String {
    let tail = if meter.is_empty() {
        format!("  {}", duration(elapsed_ms))
    } else {
        format!("  {meter}  {}", duration(elapsed_ms))
    };
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
pub fn progress_line(label: &str, meter: &str, elapsed_ms: u64) -> String {
    if meter.is_empty() {
        format!("  ... {label}: still running, {}", duration(elapsed_ms))
    } else {
        format!("  ... {label}: {meter}, {}", duration(elapsed_ms))
    }
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
            pct: None,
            step_ms: 12_000,
            run_ms: 41_000,
        };
        assert_eq!(
            title(&working, 1),
            "⠙ nura 2/14 Check the phone over adb · 0:12 (0:41)"
        );
        let bar = Title::Working {
            n: 5,
            total: 14,
            label: "Back up",
            pct: Some(42),
            step_ms: 312_000,
            run_ms: 723_000,
        };
        assert_eq!(title(&bar, 0), "⠋ nura 5/14 Back up 42% · 5:12 (12:03)");
        let you = Title::Waiting {
            n: 7,
            total: 14,
            what: "Volume Down + Power",
            step_ms: 35_000,
            run_ms: 880_000,
        };
        assert_eq!(
            title(&you, 9),
            "⏸ nura 7/14 Volume Down + Power · 0:35 (14:40)"
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
    fn limits_read_as_seconds_or_minutes() {
        assert_eq!(limit(0), "0 s");
        assert_eq!(limit(45), "45 s");
        assert_eq!(limit(60), "1 min");
        assert_eq!(limit(900), "15 min");
        assert_eq!(limit(90), "1 min 30 s");
        assert_eq!(up_to(None), "");
        assert_eq!(
            up_to(Some(std::time::Duration::from_secs(600))),
            " (up to 10 min)"
        );
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
    fn a_skip_has_no_duration() {
        assert_eq!(
            skipped("getvar", "no phone"),
            "  [skip] getvar: no phone".to_string()
        );
    }

    #[test]
    fn the_run_ends_with_its_total_and_outcome() {
        assert_eq!(run_end(42_000, "finished"), "done in 42 s: finished");
        assert_eq!(run_end(83_000, "stopped"), "done in 1:23: stopped");
    }

    #[test]
    fn the_bar_has_24_cells_filled_in_proportion() {
        assert_eq!(bar(0, 100), "░".repeat(24));
        assert_eq!(bar(33, 100), format!("{}{}", "█".repeat(7), "░".repeat(17)));
        assert_eq!(bar(100, 100), "█".repeat(24));
        assert_eq!(bar(500, 100), "█".repeat(24));
        assert_eq!(bar(0, 0), "█".repeat(24));
    }

    #[test]
    fn a_known_total_shows_bar_quantity_percent_and_rate() {
        let m = Meter::Bytes {
            done: 100 << 20,
            total: Some(300 << 20),
        };
        assert_eq!(
            meter_text(m, 10_000, true),
            format!(
                "{}{}  100.0 MiB / 300.0 MiB  33%  10.0 MiB/s",
                "█".repeat(8),
                "░".repeat(16)
            )
        );
        assert_eq!(
            meter_text(m, 10_000, false),
            "100.0 MiB of 300.0 MiB (33%), 10.0 MiB/s"
        );
    }

    #[test]
    fn a_stream_past_its_total_drops_to_the_plain_count_never_over_100_percent() {
        let past = Meter::Bytes {
            done: 301 << 20,
            total: Some(300 << 20),
        };
        assert_eq!(meter_text(past, 10_000, true), "301.0 MiB, 30.1 MiB/s");
        let exact = Meter::Bytes {
            done: 300 << 20,
            total: Some(300 << 20),
        };
        assert_eq!(
            meter_text(exact, 10_000, true),
            format!(
                "{}  300.0 MiB / 300.0 MiB  100%  30.0 MiB/s",
                "█".repeat(24)
            )
        );
    }

    #[test]
    fn an_unknown_total_shows_only_what_was_counted() {
        let bytes = Meter::Bytes {
            done: 3 << 20,
            total: None,
        };
        assert_eq!(meter_text(bytes, 1000, true), "3.0 MiB, 3.0 MiB/s");
        let zero = Meter::Bytes {
            done: 0,
            total: None,
        };
        assert_eq!(meter_text(zero, 0, true), "0 bytes");
        assert_eq!(meter_text(Meter::None, 5000, true), "");
    }

    #[test]
    fn the_spinner_rewrites_its_line_with_a_braille_frame() {
        assert_eq!(
            spinner(0, "hashing", "", 1234, 80),
            "\r\x1b[K  ⠋ hashing  1.2 s"
        );
        assert_eq!(
            spinner(11, "hashing", "7 files", 65_000, 80),
            "\r\x1b[K  ⠙ hashing  7 files  1:05"
        );
        let frames: String = (0..10)
            .map(|t| {
                spinner(t, "x", "", 0, 80)
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
        let line = spinner(0, &label, "3 files", 1000, 40);
        let shown = line.trim_start_matches("\r\x1b[K");
        assert!(shown.chars().count() < 40, "{shown}");
        assert!(shown.ends_with("…  3 files  1.0 s"), "{shown}");
    }

    #[test]
    fn clearing_the_line_is_carriage_return_and_erase() {
        assert_eq!(clear(), "\r\x1b[K");
    }

    #[test]
    fn the_transcript_progress_line_is_plain_text() {
        assert_eq!(
            progress_line("tar stream", "1.0 GiB, 15.0 MiB/s", 70_000),
            "  ... tar stream: 1.0 GiB, 15.0 MiB/s, 1:10"
        );
        assert_eq!(
            progress_line("waiting", "", 30_000),
            "  ... waiting: still running, 30 s"
        );
        assert!(!progress_line("x", "y", 1).contains('\x1b'));
    }

    #[test]
    fn marks_keep_their_tags() {
        assert_eq!(Mark::Pass.tag(), "[ ok ]");
        assert_eq!(Mark::Fail.tag(), "[FAIL]");
        assert_eq!(Mark::Skip.tag(), "[skip]");
    }
}
