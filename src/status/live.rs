use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::Print;
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};

const TICK: Duration = Duration::from_millis(200);

pub enum Outcome {
    Quit,
    Stopped(String),
}

pub fn width() -> Option<usize> {
    terminal::size()
        .ok()
        .map(|(w, _)| usize::from(w).saturating_sub(1))
        .filter(|w| *w > 0)
}

struct Screen {
    stop: Arc<AtomicBool>,
    #[cfg(unix)]
    hooks: Vec<signal_hook::SigId>,
}

fn restore() {
    let _ = execute!(
        io::stdout(),
        EndSynchronizedUpdate,
        Show,
        LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}

impl Screen {
    fn enter() -> io::Result<Screen> {
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(unix)]
        let hooks = [
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
            signal_hook::consts::SIGINT,
        ]
        .into_iter()
        .map(|sig| signal_hook::flag::register(sig, stop.clone()))
        .collect::<io::Result<Vec<_>>>()?;
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        terminal::enable_raw_mode()?;
        if let Err(e) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(e);
        }
        Ok(Screen {
            stop,
            #[cfg(unix)]
            hooks,
        })
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
        drop(std::panic::take_hook());
        #[cfg(unix)]
        for id in self.hooks.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

fn quits(k: &KeyEvent) -> bool {
    k.kind == KeyEventKind::Press
        && match k.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => true,
            KeyCode::Char('c') => k.modifiers.contains(KeyModifiers::CONTROL),
            _ => false,
        }
}

pub fn fit(text: &str, height: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let room = height.saturating_sub(2).max(1);
    if lines.len() <= room {
        return lines.into_iter().map(str::to_string).collect();
    }
    let mut kept: Vec<String> = lines[..room - 1].iter().map(|l| l.to_string()).collect();
    kept.push(format!(
        "\x1b[2m… {} more lines, make the terminal taller to see them\x1b[0m",
        lines.len() - (room - 1)
    ));
    kept
}

fn draw(text: &str, footer: &str) -> io::Result<()> {
    let height = terminal::size().map(|(_, h)| usize::from(h)).unwrap_or(24);
    let lines = fit(text, height);
    let mut out = io::stdout().lock();
    queue!(out, BeginSynchronizedUpdate)?;
    for (row, line) in lines.iter().enumerate() {
        let row = u16::try_from(row).unwrap_or(u16::MAX);
        queue!(
            out,
            MoveTo(0, row),
            Print(line),
            Clear(ClearType::UntilNewLine)
        )?;
    }
    let below = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    queue!(out, MoveTo(0, below), Clear(ClearType::FromCursorDown))?;
    let last = u16::try_from(height.saturating_sub(1)).unwrap_or(u16::MAX);
    queue!(out, MoveTo(0, last), Print(footer), EndSynchronizedUpdate)?;
    out.flush()
}

pub fn run(
    every: Duration,
    footer: &str,
    mut frame: impl FnMut(bool) -> Option<String>,
) -> io::Result<Outcome> {
    let Some(mut shown) = frame(false) else {
        return Ok(Outcome::Stopped(String::new()));
    };
    let screen = Screen::enter()?;
    draw(&shown, footer)?;
    let mut due = Instant::now() + every;
    loop {
        if screen.stopped() {
            return Ok(Outcome::Quit);
        }
        let wait = due.saturating_duration_since(Instant::now()).min(TICK);
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(k) if quits(&k) => return Ok(Outcome::Quit),
                Event::Resize(..) => {
                    if let Some(text) = frame(false) {
                        shown = text;
                    }
                    draw(&shown, footer)?;
                }
                _ => {}
            }
            continue;
        }
        if Instant::now() < due {
            continue;
        }
        due = Instant::now() + every;
        match frame(true) {
            Some(text) => {
                shown = text;
                draw(&shown, footer)?;
            }
            None => {
                drop(screen);
                return Ok(Outcome::Stopped(shown));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_taller_than_the_terminal_says_what_is_hidden() {
        let text: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        let lines = fit(&text, 12);
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[8], "line 9");
        assert!(lines[9].contains("21 more lines"), "{}", lines[9]);
        assert_eq!(fit("a\nb\n", 12), vec!["a", "b"]);
    }

    #[test]
    fn quit_keys() {
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        assert!(quits(&key(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert!(quits(&key(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(quits(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(!quits(&key(KeyCode::Char('c'), KeyModifiers::NONE)));
        assert!(!quits(&key(KeyCode::Char('x'), KeyModifiers::NONE)));
    }
}
