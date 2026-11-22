//! Menus and approvals. They read only a terminal, after dropping anything typed ahead.
use crate::config;
use crate::fail::Result;
use crate::log;
use crate::ui;
use std::io;
use std::sync::Mutex;
use std::time::Instant;

/// Whoever reads the terminal holds this, so two prompts never read at once.
static INPUT: Mutex<()> = Mutex::new(());

fn is_terminal() -> bool {
    // SAFETY: isatty only inspects the descriptor number; it touches no memory.
    unsafe { libc::isatty(0) == 1 }
}

pub fn require_terminal() -> Result<()> {
    if is_terminal() || config::allow_piped_input() {
        Ok(())
    } else {
        Err("gates and confirmations are read only from a terminal, and stdin is not one; run nura-install in an interactive terminal".into())
    }
}

fn discard_typed_ahead() -> Result<()> {
    if !is_terminal() {
        return Ok(());
    }
    // SAFETY: tcflush takes a descriptor and a queue selector, no pointers.
    if unsafe { libc::tcflush(0, libc::TCIFLUSH) } != 0 {
        return Err(format!(
            "discarding typed-ahead input: {}",
            io::Error::last_os_error()
        )
        .into());
    }
    Ok(())
}

/// One byte straight from the descriptor, so nothing after the line is taken from the next question.
fn read_byte() -> Result<Option<u8>> {
    let mut byte = 0u8;
    loop {
        // SAFETY: reads at most one byte into a valid, writable one-byte buffer.
        let n = unsafe { libc::read(0, std::ptr::addr_of_mut!(byte).cast(), 1) };
        match n {
            1 => return Ok(Some(byte)),
            0 => return Ok(None),
            _ => {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(format!("reading answer: {e}").into());
                }
            }
        }
    }
}

/// A line without its ending; `None` at end of input.
fn read_line() -> Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        match read_byte()? {
            None if line.is_empty() => return Ok(None),
            None | Some(b'\n') => return Ok(Some(String::from_utf8_lossy(&line).into_owned())),
            Some(b) => line.push(b),
        }
    }
}

/// `None` at end of input.
fn ask(prompt: &str) -> Result<Option<String>> {
    require_terminal()?;
    let _reading = INPUT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    discard_typed_ahead()?;
    ui::put(prompt);
    ui::asking(Some(prompt));
    let answer = read_line();
    ui::asking(None);
    let answer = answer?;
    if !is_terminal() {
        // A terminal echoes the owner's Enter; piped input leaves the prompt line open.
        ui::put("\n");
    }
    Ok(answer.map(|line| strip_line_ending(&line).to_string()))
}

fn strip_line_ending(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// One of `words`, typed exactly; `None` on `stop` or end of input.
pub fn choose<'a>(prompt: &str, words: &[&'a str]) -> Result<Option<&'a str>> {
    loop {
        match ask(prompt)?.as_deref() {
            None => {
                log::line("choice: end of input");
                return Ok(None);
            }
            Some(a) if a.trim() == "stop" => {
                log::line("choice: stopped");
                return Ok(None);
            }
            Some(a) => {
                if let Some(word) = words.iter().find(|w| **w == a.trim()) {
                    log::line(&format!("choice: {word}"));
                    return Ok(Some(word));
                }
                log::line("choice: not one of the options, asked again");
                ui::put(&format!(
                    "That is not one of {}. Type one exactly, or `stop` to end the run.\n",
                    words.join(", ")
                ));
            }
        }
    }
}

pub struct Entry {
    /// What a piped test types, and what the transcript logs.
    pub word: String,
    pub label: String,
    pub about: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Up,
    Down,
    Enter,
    Stop,
}

#[derive(Default)]
struct Parser {
    state: Escape,
}

#[derive(Default, PartialEq, Eq)]
enum Escape {
    #[default]
    Ground,
    Esc,
    Bracket,
}

impl Parser {
    fn feed(&mut self, byte: u8) -> Option<Key> {
        match (&self.state, byte) {
            (Escape::Ground, 0x1b) => {
                self.state = Escape::Esc;
                None
            }
            (Escape::Esc, b'[' | b'O') => {
                self.state = Escape::Bracket;
                None
            }
            (Escape::Esc, _) => {
                self.state = Escape::Ground;
                Some(Key::Stop)
            }
            (Escape::Bracket, b) => {
                self.state = Escape::Ground;
                match b {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    _ => None,
                }
            }
            (Escape::Ground, b'k') => Some(Key::Up),
            (Escape::Ground, b'j') => Some(Key::Down),
            (Escape::Ground, b'\r' | b'\n') => Some(Key::Enter),
            (Escape::Ground, b'q' | 0x03 | 0x04) => Some(Key::Stop),
            _ => None,
        }
    }

    /// No byte followed an Escape: the owner pressed Escape itself.
    fn timeout(&mut self) -> Option<Key> {
        (std::mem::take(&mut self.state) == Escape::Esc).then_some(Key::Stop)
    }
}

const STOP_LABEL: &str = "Stop the run (nothing is written)";

/// The menu as it is drawn: one line per entry, labels aligned, then a blank line and the Stop line
/// (no radio marker, a cross instead). Nothing is selected until an arrow is pressed. The selected line has the cursor and is bold; an unselected
/// Stop is dim. Each line is cut to `cols` columns.
fn render_menu(
    entries: &[Entry],
    selected: Option<usize>,
    cols: usize,
    styled: bool,
) -> Vec<String> {
    let width = entries
        .iter()
        .map(|e| e.label.chars().count())
        .max()
        .unwrap_or(0);
    let fit = |mut text: String| {
        if text.chars().count() > cols {
            text = text.chars().take(cols.saturating_sub(1)).collect();
            text.push('\u{2026}');
        }
        text
    };
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let (cursor, marker) = if selected == Some(i) {
            ("\u{276f}", "(\u{2022})")
        } else {
            (" ", "( )")
        };
        let mut text = format!("  {cursor} {marker} {:<width$}", e.label);
        if !e.about.is_empty() {
            text.push_str(&format!("  {}", e.about));
        }
        let text = fit(text.trim_end().to_string());
        lines.push(if selected == Some(i) && styled {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text
        });
    }
    lines.push(String::new());
    let on = selected == Some(entries.len());
    let cursor = if on { "\u{276f}" } else { " " };
    let text = fit(format!("  {cursor} \u{2717} {STOP_LABEL}"));
    lines.push(match (on, styled) {
        (true, true) => format!("\x1b[1m{text}\x1b[0m"),
        (false, true) => format!("\x1b[2m{text}\x1b[0m"),
        (_, false) => text,
    });
    lines
}

static SAVED: Mutex<Option<libc::termios>> = Mutex::new(None);

/// Gives the terminal back as it was and shows the cursor; safe to call more than once, and from
/// the interrupt path.
pub fn restore_terminal() {
    let saved = SAVED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(saved) = saved {
        // SAFETY: restores a termios this process read from descriptor 0.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved) };
        ui::put("\x1b[?25h");
    }
}

struct Raw;

impl Raw {
    fn enter() -> Result<Raw> {
        // SAFETY: tcgetattr fills a termios we own; tcsetattr reads one.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return Err(
                    format!("reading the terminal mode: {}", io::Error::last_os_error()).into(),
                );
            }
            *SAVED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(t);
            t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
            t.c_cc[libc::VMIN] = 1;
            t.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(0, libc::TCSAFLUSH, &t) != 0 {
                let e = io::Error::last_os_error();
                restore_terminal();
                return Err(format!("setting the terminal mode: {e}").into());
            }
        }
        ui::put("\x1b[?25l");
        Ok(Raw)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn columns() -> usize {
    // SAFETY: TIOCGWINSZ fills a winsize we own.
    unsafe {
        let mut w: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut w) == 0 && w.ws_col > 0 {
            return usize::from(w.ws_col);
        }
    }
    80
}

fn byte_within(ms: i32) -> Result<Option<Option<u8>>> {
    let mut fd = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for the duration of the call.
    let ready = unsafe { libc::poll(&mut fd, 1, ms) };
    if ready < 0 {
        let e = io::Error::last_os_error();
        return if e.kind() == io::ErrorKind::Interrupted {
            Ok(None)
        } else {
            Err(format!("watching the input: {e}").into())
        };
    }
    if ready == 0 {
        return Ok(None);
    }
    read_byte().map(Some)
}

fn draw(lines: &[String], again: bool) {
    let mut text = String::new();
    if again {
        text.push_str(&format!("\x1b[{}A", lines.len()));
    }
    for line in lines {
        text.push_str(&format!("\r\x1b[2K{line}\n"));
    }
    ui::put(&text);
}

/// Whether the terminal takes styling: not with `NO_COLOR` set or a dumb terminal.
fn styled() -> bool {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
    !no_color && !dumb
}

/// The hint line; while Enter is on its security delay "enter select" is dim, or says so in words
/// where there is no styling.
fn render_hint(locked: bool, styled: bool) -> String {
    let enter = match (locked, styled) {
        (false, _) => "enter select".to_string(),
        (true, true) => "\x1b[2menter select\x1b[0m".to_string(),
        (true, false) => "(enter available shortly)".to_string(),
    };
    format!("  \u{2191}/\u{2193} choose \u{b7} {enter}")
}

/// An arrow-key menu with Stop as its last entry, and no cursor until the first arrow: the index
/// chosen, `None` for Stop (also Escape, `q`, Ctrl-C and end of input). For the first seconds after it is drawn it takes no keys but
/// Ctrl-C, and what was typed meanwhile is dropped. Piped input (only the test build allows it)
/// takes the typed word, with no lock.
pub fn menu(entries: &[Entry]) -> Result<Option<usize>> {
    require_terminal()?;
    if !is_terminal() {
        let width = entries.iter().map(|e| e.word.len()).max().unwrap_or(0);
        for e in entries {
            ui::say(&format!("  {:<width$} {}", e.word, e.about));
        }
        let words: Vec<&str> = entries.iter().map(|e| e.word.as_str()).collect();
        let prompt = format!("Type {}, or `stop`: ", words.join(", "));
        return Ok(choose(&prompt, &words)?.and_then(|w| entries.iter().position(|e| e.word == w)));
    }
    let _reading = INPUT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    discard_typed_ahead()?;
    ui::asking(Some("`choose an option`"));
    let result = pick(entries);
    ui::asking(None);
    result
}

/// One option to approve, with Stop; `word` is what a piped test types.
pub fn approve(word: &str, label: &str) -> Result<bool> {
    let entry = Entry {
        word: word.to_string(),
        label: label.to_string(),
        about: String::new(),
    };
    Ok(menu(&[entry])?.is_some())
}

fn pick(entries: &[Entry]) -> Result<Option<usize>> {
    let cols = columns();
    let last = entries.len();
    let styled = styled();
    let mut selected: Option<usize> = None;
    let (first_delay, move_delay) = (
        config::security_delay()?,
        config::security_delay_after_move()?,
    );
    let opened = Instant::now();
    let mut enter_ok_at = opened + first_delay;
    let mut parser = Parser::default();
    let raw = Raw::enter()?;
    let locked = |ok_at: Instant| Instant::now() < ok_at;
    let block = |locked: bool, selected: Option<usize>| {
        let mut lines = vec![render_hint(locked, styled)];
        lines.extend(render_menu(entries, selected, cols, styled));
        lines
    };
    let mut shown = locked(enter_ok_at);
    let mut lines = block(shown, selected);
    draw(&lines, false);
    let mut key = None;
    while key.is_none() {
        let now = locked(enter_ok_at);
        if now != shown {
            shown = now;
            lines[0] = render_hint(shown, styled);
            ui::put(&format!(
                "\x1b[{n}A\r\x1b[2K{}\x1b[{n}B\r",
                lines[0],
                n = lines.len()
            ));
        }
        let byte = if parser.state == Escape::Esc {
            byte_within(50)?
        } else {
            byte_within(100)?
        };
        let pressed = match byte {
            None => parser.timeout(),
            Some(None) | Some(Some(0x03)) => Some(Key::Stop),
            Some(Some(b)) => parser.feed(b),
        };
        match pressed {
            Some(Key::Up) => {
                selected = Some(selected.map_or(last, |i| (i + last) % (last + 1)));
                enter_ok_at = enter_ok_at.max(Instant::now() + move_delay);
            }
            Some(Key::Down) => {
                selected = Some(selected.map_or(0, |i| (i + 1) % (last + 1)));
                enter_ok_at = enter_ok_at.max(Instant::now() + move_delay);
            }
            Some(Key::Enter) if selected.is_none() || locked(enter_ok_at) => continue,
            Some(k) => key = Some(k),
            None => continue,
        }
        if key.is_none() {
            shown = locked(enter_ok_at);
            lines = block(shown, selected);
            draw(&lines, true);
        }
    }
    drop(raw);
    let chosen = selected.filter(|i| key == Some(Key::Enter) && *i < last);
    let (word, shown) = chosen.map_or(("stop", "Stop the run"), |i| {
        (entries[i].word.as_str(), entries[i].label.as_str())
    });
    ui::put(&format!("\x1b[{}A\x1b[J  chosen: {shown}\n", lines.len()));
    log::line(&format!(
        "choice: {word} (menu open {:.1} s)",
        opened.elapsed().as_secs_f64()
    ));
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_line_ending_is_trimmed() {
        assert_eq!(strip_line_ending("install nura\n"), "install nura");
        assert_eq!(strip_line_ending("install nura\r\n"), "install nura");
        assert_eq!(strip_line_ending("install nura \n"), "install nura ");
        assert_eq!(strip_line_ending(" install nura\n"), " install nura");
        assert_eq!(strip_line_ending("install nura"), "install nura");
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        ["Phosh", "Plasma Mobile"]
            .iter()
            .map(|l| Entry {
                word: l.to_lowercase(),
                label: l.to_string(),
                about: if *l == "Phosh" {
                    "simple".into()
                } else {
                    String::new()
                },
            })
            .collect()
    }

    #[test]
    fn the_menu_marks_the_selection_and_sets_stop_apart() {
        assert_eq!(
            render_menu(&entries(), Some(0), 80, true),
            [
                "\x1b[1m  \u{276f} (\u{2022}) Phosh          simple\x1b[0m",
                "    ( ) Plasma Mobile",
                "",
                "\x1b[2m    \u{2717} Stop the run (nothing is written)\x1b[0m"
            ]
        );
        assert_eq!(
            render_menu(&entries(), Some(2), 80, true),
            [
                "    ( ) Phosh          simple",
                "    ( ) Plasma Mobile",
                "",
                "\x1b[1m  \u{276f} \u{2717} Stop the run (nothing is written)\x1b[0m"
            ]
        );
    }

    #[test]
    fn the_hint_dims_enter_while_it_is_on_its_security_delay() {
        assert_eq!(
            render_hint(true, true),
            "  \u{2191}/\u{2193} choose \u{b7} \x1b[2menter select\x1b[0m"
        );
        assert_eq!(
            render_hint(false, true),
            "  \u{2191}/\u{2193} choose \u{b7} enter select"
        );
        assert_eq!(
            render_hint(true, false),
            "  \u{2191}/\u{2193} choose \u{b7} (enter available shortly)"
        );
        assert_eq!(
            render_hint(false, false),
            "  \u{2191}/\u{2193} choose \u{b7} enter select"
        );
    }

    #[test]
    fn without_styling_the_menu_has_no_escapes() {
        let lines = render_menu(&entries(), Some(0), 80, false);
        assert_eq!(lines[0], "  \u{276f} (\u{2022}) Phosh          simple");
        assert_eq!(lines[3], "    \u{2717} Stop the run (nothing is written)");
    }

    #[test]
    fn nothing_is_highlighted_before_an_arrow_is_pressed() {
        assert_eq!(
            render_menu(&entries(), None, 80, true),
            [
                "    ( ) Phosh          simple",
                "    ( ) Plasma Mobile",
                "",
                "\x1b[2m    \u{2717} Stop the run (nothing is written)\x1b[0m"
            ]
        );
    }

    #[test]
    fn a_long_line_is_cut_to_the_width_with_an_ellipsis() {
        let lines = render_menu(&entries(), Some(1), 12, true);
        assert_eq!(lines[0], "    ( ) Pho\u{2026}");
        assert_eq!(lines[0].chars().count(), 12);
    }

    #[test]
    fn keys_decode_arrows_vi_keys_enter_and_every_way_to_stop() {
        let keys = |bytes: &[u8]| {
            let mut p = Parser::default();
            bytes.iter().filter_map(|b| p.feed(*b)).collect::<Vec<_>>()
        };
        assert_eq!(keys(b"\x1b[B\x1b[A"), [Key::Down, Key::Up]);
        assert_eq!(keys(b"\x1bOBjk"), [Key::Down, Key::Down, Key::Up]);
        assert_eq!(keys(b"\r"), [Key::Enter]);
        assert_eq!(keys(b"\n"), [Key::Enter]);
        assert_eq!(keys(b"q"), [Key::Stop]);
        assert_eq!(keys(&[3]), [Key::Stop]);
        assert_eq!(keys(b"\x1bx"), [Key::Stop]);
        assert_eq!(keys(b"\x1b[Cz"), []);
        let mut p = Parser::default();
        assert_eq!(p.feed(0x1b), None);
        assert_eq!(p.timeout(), Some(Key::Stop));
        assert_eq!(p.timeout(), None);
    }
}
