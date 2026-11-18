//! A shell or another program on this machine in a pty of its own.

use std::{
    env,
    io::{ErrorKind, Read, Write},
    path::Path,
    thread,
};

use anyhow::{Context, Result, anyhow};
use log::{debug, info, warn};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::TerminalSize;

/// The byte the Ctrl C key sends. The pty turns it into the interrupt
/// signal for what runs in front.
pub(crate) const CTRL_C: u8 = 3;

/// The login shell of the user or another program, started in a pty.
/// Dropping it closes the pty, which hangs the program up, the signal of a
/// closed terminal window, and a shell passes it on to what runs in it.
pub struct LocalShell {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
}

impl LocalShell {
    /// Starts the login shell of the user in `folder`. `printed` gets every
    /// piece it prints, on a thread of its own, and `ended` runs once after
    /// the shell is gone, with its exit code. A shell that a signal ended
    /// has none.
    pub fn start(
        folder: &Path,
        size: TerminalSize,
        printed: impl FnMut(Vec<u8>) + Send + 'static,
        ended: impl FnOnce(Option<u32>) + Send + 'static,
    ) -> Result<Self> {
        let (program, args) = login_shell();
        Self::spawn(&program, &args, folder, size, printed, ended)
    }

    /// Runs `command`, a program and its arguments, in `folder`, with the
    /// pty as its terminal. The program is found on the `PATH` of this
    /// process. `printed` and `ended` are those of `start`.
    pub fn run(
        command: &[String],
        folder: &Path,
        size: TerminalSize,
        printed: impl FnMut(Vec<u8>) + Send + 'static,
        ended: impl FnOnce(Option<u32>) + Send + 'static,
    ) -> Result<Self> {
        let (program, args) = command.split_first().context("no program to run")?;
        Self::spawn(program, args, folder, size, printed, ended)
    }

    fn spawn(
        program: &str,
        args: &[String],
        folder: &Path,
        size: TerminalSize,
        mut printed: impl FnMut(Vec<u8>) + Send + 'static,
        ended: impl FnOnce(Option<u32>) + Send + 'static,
    ) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(|err| anyhow!("cannot open a terminal: {err}"))?;
        let mut builder = CommandBuilder::new(program);
        builder.args(args);
        builder.cwd(folder);
        if env::var_os("TERM").is_none() {
            builder.env("TERM", "xterm-256color");
        }
        let mut child = pair
            .slave
            .spawn_command(builder)
            .map_err(|err| anyhow!("cannot start {program}: {err}"))?;
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| anyhow!("cannot read the terminal: {err}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| anyhow!("cannot write to the terminal: {err}"))?;
        info!("started {program} in {}", folder.display());

        let name = program.to_string();
        thread::Builder::new()
            .name("terminal shell".into())
            .spawn(move || {
                let mut buffer = vec![0; 64 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => printed(buffer[..read].to_vec()),
                        Err(err) if err.kind() == ErrorKind::Interrupted => {}
                        Err(err) => {
                            debug!("the stream of {name} ended: {err}");
                            break;
                        }
                    }
                }
                // The wait takes the ended program out of the process table.
                let code = match child.wait() {
                    Ok(status) => {
                        info!("{name} ended, {status:?}");
                        status.signal().is_none().then(|| status.exit_code())
                    }
                    Err(err) => {
                        warn!("the end of {name} gave no status: {err}");
                        None
                    }
                };
                ended(code);
            })
            .context("start the reader of the shell")?;

        Ok(Self {
            master: pair.master,
            writer,
        })
    }

    /// Keys, a paste or an answer for the shell.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("write to the shell")?;
        self.writer.flush().context("write to the shell")
    }

    /// The screen of the shell got another size.
    pub fn resize(&self, size: TerminalSize) -> Result<()> {
        self.master
            .resize(pty_size(size))
            .map_err(|err| anyhow!("cannot resize the terminal: {err}"))
    }
}

fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows:         size.lines,
        cols:         size.columns,
        pixel_width:  size.cell_width.saturating_mul(size.columns),
        pixel_height: size.cell_height.saturating_mul(size.lines),
    }
}

/// The login shell of the user and its arguments. On Windows it is
/// PowerShell 7.
fn login_shell() -> (String, Vec<String>) {
    if cfg!(windows) {
        return ("pwsh".to_string(), vec!["-NoLogo".to_string()]);
    }
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    (shell, vec!["-l".to_string()])
}

#[cfg(all(test, unix))]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    #[test]
    fn a_shell_runs_a_command_and_ends_on_exit() -> Result<()> {
        let (printed, output) = mpsc::channel::<Vec<u8>>();
        let (ended, end) = mpsc::channel::<()>();
        let mut shell = LocalShell::start(
            &env::temp_dir(),
            size(),
            move |bytes| printed.send(bytes).expect("the test reads the output"),
            move |_| ended.send(()).expect("the test waits for the end"),
        )?;
        // The sum shows the shell ran the line, an echo of the typed keys
        // holds no 42.
        shell.write(b"echo codeloom-$((40+2))\n")?;
        let mut seen = String::new();
        while !seen.contains("codeloom-42") {
            let bytes = output.recv_timeout(Duration::from_secs(10)).context("no output")?;
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
        shell.write(b"exit\n")?;
        end.recv_timeout(Duration::from_secs(10)).context("the shell did not end")?;
        Ok(())
    }

    fn size() -> TerminalSize {
        TerminalSize {
            columns:     80,
            lines:       24,
            cell_width:  8,
            cell_height: 16,
        }
    }

    /// Runs `script` with `sh` and gives what it printed and its exit code.
    fn run_sh(script: &str) -> Result<(String, Option<u32>)> {
        let (printed, output) = mpsc::channel::<Vec<u8>>();
        let (ended, end) = mpsc::channel::<Option<u32>>();
        let command = ["sh", "-c", script].map(str::to_string);
        // Held until the end, a dropped shell hangs the program up.
        let shell = LocalShell::run(
            &command,
            &env::temp_dir(),
            size(),
            move |bytes| printed.send(bytes).expect("the test reads the output"),
            move |code| ended.send(code).expect("the test waits for the end"),
        )?;
        let code = end.recv_timeout(Duration::from_secs(10)).context("the program did not end")?;
        drop(shell);
        let seen: Vec<u8> = output.try_iter().flatten().collect();
        Ok((String::from_utf8_lossy(&seen).into_owned(), code))
    }

    #[test]
    fn a_program_runs_and_gives_its_exit_code() -> Result<()> {
        let (seen, code) = run_sh("echo ran-$((40+2)); exit 7")?;
        assert!(seen.contains("ran-42"), "printed {seen:?}");
        assert_eq!(code, Some(7));
        Ok(())
    }

    #[test]
    fn a_program_sees_a_terminal() -> Result<()> {
        let (seen, code) = run_sh("test -t 0 && test -t 1 && echo a-terminal")?;
        assert!(seen.contains("a-terminal"), "printed {seen:?}");
        assert_eq!(code, Some(0));
        Ok(())
    }

    #[test]
    fn a_program_ended_by_a_signal_has_no_exit_code() -> Result<()> {
        let (_, code) = run_sh("kill -KILL $$")?;
        assert_eq!(code, None);
        Ok(())
    }

    #[test]
    fn ctrl_c_ends_a_program() -> Result<()> {
        let (printed, output) = mpsc::channel::<Vec<u8>>();
        let (ended, end) = mpsc::channel::<Option<u32>>();
        let command = ["sh", "-c", "echo waits; sleep 60"].map(str::to_string);
        let mut shell = LocalShell::run(
            &command,
            &env::temp_dir(),
            size(),
            move |bytes| printed.send(bytes).expect("the test reads the output"),
            move |code| ended.send(code).expect("the test waits for the end"),
        )?;
        // The program runs once it printed, a Ctrl C before that finds no
        // program to end.
        let mut seen = String::new();
        while !seen.contains("waits") {
            let bytes = output.recv_timeout(Duration::from_secs(10)).context("no output")?;
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
        shell.write(&[CTRL_C])?;
        end.recv_timeout(Duration::from_secs(10))
            .context("Ctrl C did not end the program")?;
        Ok(())
    }

    #[test]
    fn no_program_is_an_error() {
        let result = LocalShell::run(&[], &env::temp_dir(), size(), |_| {}, |_| {});
        assert!(result.is_err());
    }
}
