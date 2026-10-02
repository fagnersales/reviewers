use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const BAR: &str = "│";
pub const BAR_START: &str = "┌";
pub const BAR_END: &str = "└";
pub const STEP_ACTIVE: &str = "◆";
pub const STEP_DONE: &str = "◇";
pub const STEP_CANCEL: &str = "■";
pub const SPINNER: [&str; 4] = ["◒", "◐", "◓", "◑"];
/// The logo, as text: a selected box, the same mark the site and the favicon use.
pub const LOGO: &str = "[◆]";

pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

pub fn interactive() -> bool {
    std::io::stdout().is_terminal() && std::io::stdin().is_terminal()
}

fn colors() -> bool {
    static COLORS: OnceLock<bool> = OnceLock::new();
    *COLORS.get_or_init(|| std::env::var_os("NO_COLOR").is_none() && stdout_is_tty())
}

fn paint(code: &str, text: &str) -> String {
    if colors() { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
}

pub fn bold(text: &str) -> String {
    paint("1", text)
}
pub fn dim(text: &str) -> String {
    paint("2", text)
}
pub fn red(text: &str) -> String {
    paint("31", text)
}
pub fn green(text: &str) -> String {
    paint("32", text)
}
pub fn yellow(text: &str) -> String {
    paint("33", text)
}
pub fn magenta(text: &str) -> String {
    paint("35", text)
}
pub fn cyan(text: &str) -> String {
    paint("36", text)
}
pub fn gray(text: &str) -> String {
    paint("90", text)
}
pub fn strike(text: &str) -> String {
    paint("9;2", text)
}

pub fn bar() -> String {
    gray(BAR)
}

pub fn columns() -> usize {
    terminal::size().map(|(width, _)| width as usize).unwrap_or(100)
}

pub fn terminal_rows() -> usize {
    terminal::size().map(|(_, height)| height as usize).unwrap_or(30)
}

fn escape_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&0x1b) || bytes.get(start + 1) != Some(&b'[') {
        return None;
    }
    (start + 2..bytes.len()).find(|&index| bytes[index].is_ascii_alphabetic()).map(|index| index + 1)
}

pub fn visible_len(text: &str) -> usize {
    let mut count = 0;
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = escape_end(text, index) {
            index = end;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or(' ');
        count += 1;
        index += character.len_utf8();
    }
    count
}

/// Cuts a styled line to the width without splitting an escape code, so nothing wraps.
pub fn fit(text: &str, width: usize) -> String {
    if visible_len(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut count = 0;
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = escape_end(text, index) {
            out.push_str(&text[index..end]);
            index = end;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or(' ');
        if count + 1 >= width {
            out.push('…');
            out.push_str("\x1b[0m");
            return out;
        }
        out.push(character);
        count += 1;
        index += character.len_utf8();
    }
    out
}

pub fn pad(text: &str, width: usize) -> String {
    let length = visible_len(text);
    if length >= width { text.to_string() } else { format!("{text}{}", " ".repeat(width - length)) }
}

pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    format!("{}…", text.chars().take(width.saturating_sub(1)).collect::<String>())
}

/// A line inside the gutter.
pub fn line(text: &str) {
    if text.is_empty() {
        println!("{}", bar());
    } else {
        println!("{}", fit(&format!("{}  {text}", bar()), columns().saturating_sub(1)));
    }
}

pub fn intro(title: &str) {
    println!("{}  {title}", gray(BAR_START));
}

pub fn step(text: &str) {
    println!("{}\n{}  {text}", bar(), green(STEP_DONE));
}

pub fn outro(text: &str) {
    println!("{}\n{}  {text}\n", bar(), gray(BAR_END));
}

pub fn cancelled(text: &str) {
    println!("{}\n{}  {text}\n", bar(), red(STEP_CANCEL));
}

/// A block of lines redrawn in place: the cursor climbs back over the last
/// frame and clears it. Lines are cut to the width; a wrapped line would throw
/// the climb off by one.
pub struct LiveRegion {
    height: usize,
}

impl LiveRegion {
    pub fn new() -> LiveRegion {
        print!("\x1b[?25l");
        LiveRegion { height: 0 }
    }

    pub fn update(&mut self, lines: &[String]) {
        let width = columns().saturating_sub(1);
        let mut frame = String::new();
        if self.height > 0 {
            frame.push_str(&format!("\x1b[{}A\r\x1b[0J", self.height));
        }
        for line in lines {
            frame.push_str(&fit(line, width));
            frame.push('\n');
        }
        print!("{frame}");
        let _ = std::io::stdout().flush();
        self.height = lines.len();
    }

    pub fn finish(mut self, lines: &[String]) {
        self.update(lines);
        restore_cursor();
    }
}

pub fn restore_cursor() {
    if stdout_is_tty() {
        print!("\x1b[?25h");
        let _ = std::io::stdout().flush();
    }
}

/// clack's spinner on a terminal; a start and a finish line in a log, where frames would pile up.
pub struct Spinner {
    message: Arc<Mutex<String>>,
    running: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    pub fn start(message: &str) -> Spinner {
        let shared = Arc::new(Mutex::new(message.to_string()));
        let running = Arc::new(AtomicBool::new(true));
        let handle = if stdout_is_tty() {
            println!("{}", bar());
            print!("\x1b[?25l");
            let shared = Arc::clone(&shared);
            let running = Arc::clone(&running);
            Some(std::thread::spawn(move || {
                let started = Instant::now();
                let mut frame = 0;
                while running.load(Ordering::Relaxed) {
                    let text = shared.lock().map(|text| text.clone()).unwrap_or_default();
                    let elapsed = crate::util::duration(started.elapsed().as_millis() as u64);
                    let line = fit(&format!("{}  {text} {}", magenta(SPINNER[frame % 4]), dim(&format!("[{elapsed}]"))), columns().saturating_sub(1));
                    print!("\r\x1b[2K{line}");
                    let _ = std::io::stdout().flush();
                    frame += 1;
                    std::thread::sleep(Duration::from_millis(120));
                }
            }))
        } else {
            println!("{}\n{}  {message}", bar(), cyan(STEP_ACTIVE));
            None
        };
        Spinner { message: shared, running, handle }
    }

    pub fn message(&self, text: &str) {
        if let Ok(mut message) = self.message.lock() {
            *message = text.to_string();
        }
    }

    fn halt(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
            print!("\r\x1b[2K");
            restore_cursor();
        }
    }

    pub fn stop(mut self, text: &str) {
        self.halt();
        println!("{}  {text}", green(STEP_DONE));
    }

    pub fn fail(mut self, text: &str) {
        self.halt();
        println!("{}  {text}", red(STEP_CANCEL));
    }
}

struct RawMode;

impl RawMode {
    fn enter() -> Option<RawMode> {
        terminal::enable_raw_mode().ok().map(|_| RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        restore_cursor();
    }
}

/// Redraws a prompt in place while raw mode is on (where `\n` doesn't return the carriage).
struct Frame {
    height: usize,
}

impl Frame {
    fn draw(&mut self, text: &str) {
        let width = columns().saturating_sub(1);
        let lines: Vec<String> = text.lines().map(|line| fit(line, width)).collect();
        let mut out = String::new();
        if self.height > 1 {
            out.push_str(&format!("\x1b[{}A", self.height - 1));
        }
        out.push_str("\r\x1b[0J");
        out.push_str(&lines.join("\r\n"));
        print!("{out}");
        let _ = std::io::stdout().flush();
        self.height = lines.len();
    }
}

enum Key {
    Up,
    Down,
    Toggle,
    All,
    Invert,
    Enter,
    Cancel,
    Other,
}

fn next_key() -> Key {
    loop {
        let Ok(Event::Key(KeyEvent { code, modifiers, kind, .. })) = event::read() else {
            continue;
        };
        if kind != KeyEventKind::Press {
            continue;
        }
        return match code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Left => Key::Up,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Right | KeyCode::Tab => Key::Down,
            KeyCode::Char(' ') => Key::Toggle,
            KeyCode::Char('a') => Key::All,
            KeyCode::Char('i') => Key::Invert,
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Cancel,
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => Key::Cancel,
            _ => Key::Other,
        };
    }
}

pub struct PickRow {
    pub label: String,
    pub detail: String,
    pub selected: bool,
}

/// Every row shows its numbers, and the answer collapses to one line ("All 12 repos").
pub fn multiselect(message: &str, rows: &[PickRow], noun: &str, minimum: usize) -> Option<Vec<usize>> {
    let mut picked: Vec<bool> = rows.iter().map(|row| row.selected).collect();
    let mut cursor = 0usize;
    let mut error: Option<String> = None;
    let label_width = rows.iter().map(|row| row.label.chars().count()).max().unwrap_or(0).min(44) + 2;
    let summary = |picked: &[bool]| {
        let chosen: Vec<&str> = rows.iter().zip(picked).filter(|(_, on)| **on).map(|(row, _)| row.label.as_str()).collect();
        if chosen.len() == rows.len() {
            format!("All {}", crate::util::plural(rows.len(), noun))
        } else if chosen.is_empty() {
            format!("No {noun}s")
        } else {
            let more = if chosen.len() > 4 { ", …" } else { "" };
            format!("{} of {} · {}{more}", chosen.len(), crate::util::plural(rows.len(), noun), chosen[..chosen.len().min(4)].join(", "))
        }
    };
    let _raw = RawMode::enter()?;
    print!("\x1b[?25l");
    println!("{}\r", bar());
    let mut frame = Frame { height: 0 };
    loop {
        let edge = if error.is_some() { yellow(BAR) } else { cyan(BAR) };
        let visible = rows.len().min(terminal_rows().saturating_sub(9).max(5));
        let start = cursor.saturating_sub(visible / 2).min(rows.len().saturating_sub(visible));
        let mut text = format!("{}  {message}\n", cyan(STEP_ACTIVE));
        if start > 0 {
            text.push_str(&format!("{edge}  {}\n", dim(&format!("↑ {start} more"))));
        }
        for (index, row) in rows.iter().enumerate().skip(start).take(visible) {
            let active = index == cursor;
            let box_mark = if picked[index] {
                green("◼")
            } else if active {
                cyan("◻")
            } else {
                dim("◻")
            };
            let label = pad(&truncate(&row.label, label_width - 2), label_width);
            let label = if active { label } else { dim(&label) };
            text.push_str(&format!("{edge}  {box_mark} {label}{}\n", dim(&row.detail)));
        }
        let below = rows.len().saturating_sub(start + visible);
        if below > 0 {
            text.push_str(&format!("{edge}  {}\n", dim(&format!("↓ {below} more"))));
        }
        let count = picked.iter().filter(|on| **on).count();
        text.push_str(&format!("{edge}\n{edge}  {}\n", dim(&format!("{count} of {} · ↑/↓ move · space toggle · a all · enter confirm", rows.len()))));
        if let Some(message) = &error {
            text.push_str(&format!("{}  {}\n", yellow(BAR), yellow(message)));
        }
        text.push_str(&(if error.is_some() { yellow(BAR_END) } else { cyan(BAR_END) }));
        frame.draw(&text);
        match next_key() {
            Key::Up => cursor = if cursor == 0 { rows.len() - 1 } else { cursor - 1 },
            Key::Down => cursor = (cursor + 1) % rows.len(),
            Key::Toggle => picked[cursor] = !picked[cursor],
            Key::All => {
                let all = picked.iter().all(|on| *on);
                picked.iter_mut().for_each(|on| *on = !all);
            }
            Key::Invert => picked.iter_mut().for_each(|on| *on = !*on),
            Key::Enter => {
                if picked.iter().filter(|on| **on).count() < minimum {
                    error = Some(format!("Pick at least {minimum}."));
                    continue;
                }
                frame.draw(&format!("{}  {message}\n{}  {}", green(STEP_DONE), bar(), dim(&summary(&picked))));
                print!("\r\n");
                return Some(picked.iter().enumerate().filter(|(_, on)| **on).map(|(index, _)| index).collect());
            }
            Key::Cancel => {
                frame.draw(&format!("{}  {message}\n{}  {}", red(STEP_CANCEL), bar(), strike("cancelled")));
                print!("\r\n");
                return None;
            }
            Key::Other => {}
        }
        error = None;
    }
}

pub struct Choice {
    pub label: String,
    pub hint: String,
}

pub fn select(message: &str, choices: &[Choice], initial: usize) -> Option<usize> {
    let _raw = RawMode::enter()?;
    print!("\x1b[?25l");
    println!("{}\r", bar());
    let mut cursor = initial.min(choices.len().saturating_sub(1));
    let mut frame = Frame { height: 0 };
    loop {
        let mut text = format!("{}  {message}\n", cyan(STEP_ACTIVE));
        for (index, choice) in choices.iter().enumerate() {
            let line = if index == cursor {
                format!("{} {}{}", green("●"), choice.label, if choice.hint.is_empty() { String::new() } else { dim(&format!(" ({})", choice.hint)) })
            } else {
                format!("{} {}", dim("○"), dim(&choice.label))
            };
            text.push_str(&format!("{}  {line}\n", cyan(BAR)));
        }
        text.push_str(&format!("{}  {}\n{}", cyan(BAR), dim("↑/↓ move · enter confirm"), cyan(BAR_END)));
        frame.draw(&text);
        match next_key() {
            Key::Up => cursor = if cursor == 0 { choices.len() - 1 } else { cursor - 1 },
            Key::Down => cursor = (cursor + 1) % choices.len(),
            Key::Enter => {
                frame.draw(&format!("{}  {message}\n{}  {}", green(STEP_DONE), bar(), dim(&choices[cursor].label)));
                print!("\r\n");
                return Some(cursor);
            }
            Key::Cancel => {
                frame.draw(&format!("{}  {message}\n{}  {}", red(STEP_CANCEL), bar(), strike("cancelled")));
                print!("\r\n");
                return None;
            }
            _ => {}
        }
    }
}

/// A line typed without showing it, for keys. None when cancelled.
pub fn secret(message: &str) -> Option<String> {
    let _raw = RawMode::enter()?;
    println!("{}\r", bar());
    let mut value = String::new();
    let mut frame = Frame { height: 0 };
    loop {
        let typed = if value.is_empty() { dim("paste it, then enter") } else { dim(&"•".repeat(value.chars().count().min(48))) };
        frame.draw(&format!("{}  {message}\n{}  {typed}\n{}", cyan(STEP_ACTIVE), cyan(BAR), cyan(BAR_END)));
        let Ok(Event::Key(KeyEvent { code, modifiers, kind, .. })) = event::read() else {
            continue;
        };
        if kind != KeyEventKind::Press {
            continue;
        }
        match code {
            KeyCode::Enter => {
                frame.draw(&format!("{}  {message}\n{}  {}", green(STEP_DONE), bar(), dim("received")));
                print!("\r\n");
                return Some(value);
            }
            KeyCode::Esc => break,
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => break,
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(character) => value.push(character),
            _ => {}
        }
    }
    frame.draw(&format!("{}  {message}\n{}  {}", red(STEP_CANCEL), bar(), strike("cancelled")));
    print!("\r\n");
    None
}

pub fn confirm(message: &str, default_yes: bool) -> Option<bool> {
    let choices = [Choice { label: "Yes".into(), hint: String::new() }, Choice { label: "No".into(), hint: String::new() }];
    select(message, &choices, if default_yes { 0 } else { 1 }).map(|index| index == 0)
}
