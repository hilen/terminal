//! A shell on this machine in a pty of its own.

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

/// The login shell of the user, started in a pty. Dropping it closes the
/// pty, which hangs the shell up, the signal of a closed terminal window,
/// and the shell passes it on to what runs in it.
pub struct LocalShell {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
}

impl LocalShell {
    /// Starts the login shell of the user in `folder`. `printed` gets every
    /// piece it prints, on a thread of its own, and `ended` runs once after
    /// the shell is gone.
    pub fn start(
        folder: &Path,
        size: TerminalSize,
        mut printed: impl FnMut(Vec<u8>) + Send + 'static,
        ended: impl FnOnce() + Send + 'static,
    ) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(|err| anyhow!("cannot open a terminal: {err}"))?;
        let (program, args) = login_shell();
        let mut builder = CommandBuilder::new(&program);
        builder.args(&args);
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
                            debug!("the stream of the shell ended: {err}");
                            break;
                        }
                    }
                }
                // The wait takes the ended shell out of the process table.
                match child.wait() {
                    Ok(status) => info!("the shell ended, {status:?}"),
                    Err(err) => warn!("the end of the shell gave no status: {err}"),
                }
                ended();
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
        let size = TerminalSize {
            columns:     80,
            lines:       24,
            cell_width:  8,
            cell_height: 16,
        };
        let mut shell = LocalShell::start(
            &env::temp_dir(),
            size,
            move |bytes| printed.send(bytes).expect("the test reads the output"),
            move || ended.send(()).expect("the test waits for the end"),
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
}
