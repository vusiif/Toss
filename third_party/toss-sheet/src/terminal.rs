//! Restore terminal state across setup errors, loop errors and unwinding.
use anyhow::{Context, Result};
use crossterm::{
    cursor::Show,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
struct Guard {
    active: bool,
}
impl Guard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("cannot enable terminal input")?;
        let guard = Self { active: true };
        // The guard exists before either screen/backend initialization can fail.
        execute!(io::stdout(), EnterAlternateScreen).context("cannot enter alternate screen")?;
        Ok(guard)
    }
    fn restore(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let screen = execute!(io::stdout(), LeaveAlternateScreen, Show);
        // Always attempt input restoration, even if output has already failed.
        let input = disable_raw_mode();
        match (screen, input) {
            (Ok(_), Ok(_)) => Ok(()),
            (Err(a), Err(b)) => Err(anyhow::anyhow!(
                "screen restore failed: {a}; input restore failed: {b}"
            )),
            (Err(a), _) => Err(a).context("screen restore failed"),
            (_, Err(b)) => Err(b).context("input restore failed"),
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Err(err) = self.restore() {
            eprintln!("Warning: {err:#}");
        }
    }
}
pub fn with_terminal<T>(
    run: impl FnOnce(&mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<T>,
) -> Result<T> {
    use io::IsTerminal;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("Interactive mode requires a terminal for stdin and stdout");
    }
    let mut guard = Guard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))
        .context("cannot initialize terminal backend")?;
    let result = run(&mut terminal);
    drop(terminal);
    let restored = guard.restore();
    match (result, restored) {
        (Err(error), Err(cleanup)) => {
            eprintln!("Warning: {cleanup:#}");
            Err(error)
        }
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}
