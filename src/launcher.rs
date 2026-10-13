// SPDX-License-Identifier: MPL-2.0

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use crate::{
    backend,
    model::{Backend, UpdateCommand},
};

const TERMINALS: &[(&str, &[&str])] = &[
    ("cosmic-term", &["--no-daemon", "-e"]),
    ("kgx", &["--"]),
    ("gnome-terminal", &["--wait", "--"]),
    ("konsole", &["--nofork", "-e"]),
    ("xfce4-terminal", &["--disable-server", "--execute"]),
    ("kitty", &["--"]),
    ("alacritty", &["-e"]),
    ("xterm", &["-e"]),
];

/// Open an available desktop terminal, preferring COSMIC Terminal.
pub async fn launch(backend: Backend) -> Result<(), String> {
    let executable = env::current_exe()
        .map_err(|error| format!("Could not locate the installed applet: {error}"))?;
    let mut failures = Vec::new();
    for &(terminal, execute_args) in TERMINALS {
        let Some(program) = find_executable(terminal) else {
            continue;
        };
        // COSMIC Terminal converts command arguments to UTF-8 internally.
        if terminal == "cosmic-term" && executable.to_str().is_none() {
            failures.push("cosmic-term cannot preserve this executable's non-UTF-8 path".into());
            continue;
        }
        let mut command = desktop_command(&program);
        command.args(terminal_args(execute_args, &executable, backend));
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                failures.push(format!("{terminal}: {error}"));
                continue;
            }
        };
        // Foreground flags avoid COSMIC/Konsole daemonization hiding startup errors.
        // D-Bus terminals can still hand off successfully before the window appears.
        match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
            Ok(Ok(status)) if !status.success() => {
                failures.push(format!("{terminal} exited with {status}"));
            }
            Ok(Err(error)) => failures.push(format!("{terminal}: {error}")),
            Ok(Ok(_)) => return Ok(()),
            Err(_) => {
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
                return Ok(());
            }
        }
    }
    if failures.is_empty() {
        Err("No supported terminal is installed. Install COSMIC Terminal, GNOME Console, GNOME Terminal, Konsole, Xfce Terminal, kitty, Alacritty, or xterm.".into())
    } else {
        Err(format!(
            "Could not open an update terminal: {}",
            failures.join("; ")
        ))
    }
}

fn terminal_args(prefix: &[&str], executable: &Path, backend: Backend) -> Vec<OsString> {
    let mut args: Vec<OsString> = prefix
        .iter()
        .map(|argument| OsString::from(*argument))
        .collect();
    args.extend([
        executable.as_os_str().to_owned(),
        OsString::from("--run-updates"),
        OsString::from(backend.id()),
    ]);
    args
}

fn desktop_command(program: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    let descriptors = [
        "WAYLAND_SOCKET",
        "COSMIC_NOTIFICATIONS",
        "X_PRIVILEGED_WAYLAND_SOCKET",
    ]
    .map(|name| {
        command.env_remove(name);
        env::var(name)
            .ok()
            .and_then(|value| value.parse::<libc::c_int>().ok())
            .filter(|&descriptor| descriptor >= 3)
    });
    for (name, _) in env::vars_os() {
        if name.as_bytes().starts_with(b"COSMIC_PANEL_") {
            command.env_remove(name);
        }
    }
    // SAFETY: the child closes only captured panel-private descriptors after fork;
    // close is async-signal-safe and does not affect the parent's descriptors.
    unsafe {
        command.pre_exec(move || {
            for descriptor in descriptors.into_iter().flatten() {
                libc::close(descriptor);
            }
            Ok(())
        });
    }
    command
}

fn find_executable(program: impl AsRef<OsStr>) -> Option<PathBuf> {
    let program = Path::new(program.as_ref());
    let executable = |path: &Path| {
        fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    };
    if program.components().count() > 1 {
        return executable(program).then(|| program.to_owned());
    }
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join(program))
            .find(|path| executable(path))
    })
}

/// Run the selected backend only from a real, foreground interactive terminal.
/// The update commands, including sudo authentication, inherit terminal streams.
pub fn run(selected: Backend) -> Result<(), String> {
    let mut terminal = interactive_terminal()?;
    let _signals = InterruptGuard::install()?;
    let mut lock = None;
    let result = (|| {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return Err("Do not run the applet as root. Run it as your desktop user; only system package commands use sudo.".into());
        }
        lock = Some(RunnerLock::acquire()?);
        perform_updates(selected, &mut terminal)
    })();
    match &result {
        Ok(()) => writeln!(terminal, "\nSystem update commands completed successfully."),
        Err(error) => writeln!(terminal, "\nUpdates stopped: {error}"),
    }
    .map_err(|error| format!("Could not display update status: {error}"))?;
    writeln!(terminal, "Press Enter to close this terminal.")
        .map_err(|error| format!("Could not display terminal prompt: {error}"))?;
    INTERRUPTED.store(false, Ordering::Relaxed);
    let hold = wait_for_enter(&mut terminal, false);
    drop(lock);
    result.and(hold)
}

fn interactive_terminal() -> Result<File, String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() || !io::stderr().is_terminal() {
        return Err(
            "Updates require interactive terminal stdin, stdout, and stderr; refusing to run."
                .into(),
        );
    }
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|error| format!("Updates require a controlling terminal: {error}"))?;
    // SAFETY: these queries use the live terminal descriptor and take no pointers.
    let foreground = unsafe { libc::tcgetpgrp(terminal.as_raw_fd()) };
    if foreground < 0 || foreground != unsafe { libc::getpgrp() } {
        return Err("The update runner must be in its controlling terminal's foreground.".into());
    }
    Ok(terminal)
}

fn perform_updates(selected: Backend, terminal: &mut File) -> Result<(), String> {
    let system = backend::detect()?;
    if system.backend != Some(selected) {
        return Err(format!(
            "The selected backend ({}) no longer matches this system ({}). Check for updates again.",
            selected.id(),
            system.backend.map_or("unsupported", Backend::id),
        ));
    }
    let plan = backend::update_plan(&system)?;
    if plan.steps.is_empty() {
        return Err("The selected backend provided no update commands.".into());
    }
    let sudo =
        if plan.steps.iter().any(|step| step.sudo) {
            Some(find_executable("sudo").ok_or(
                "sudo is not installed or executable; system updates cannot authenticate.",
            )?)
        } else {
            None
        };
    let mut commands = Vec::with_capacity(plan.steps.len());
    writeln!(
        terminal,
        "System updates for {} ({})",
        system.distro.name.escape_default(),
        selected.id()
    )
    .map_err(|error| format!("Could not display update plan: {error}"))?;
    writeln!(
        terminal,
        "The following commands can modify your system. Review package-manager prompts carefully."
    )
    .map_err(|error| format!("Could not display update plan: {error}"))?;
    for step in &plan.steps {
        let program = find_executable(&step.program).ok_or_else(|| {
            format!(
                "Required update program {:?} is not installed or executable.",
                step.program
            )
        })?;
        let command = update_command(step, &program, sudo.as_deref())?;
        // Debug formatting escapes control characters; this is display only, never shell input.
        write!(terminal, "  {:?}", command.get_program())
            .map_err(|error| format!("Could not display update plan: {error}"))?;
        for argument in command.get_args() {
            write!(terminal, " {argument:?}")
                .map_err(|error| format!("Could not display update plan: {error}"))?;
        }
        writeln!(terminal).map_err(|error| format!("Could not display update plan: {error}"))?;
        commands.push(command);
    }
    writeln!(terminal, "\nPress Enter to begin, or Ctrl+C to cancel.")
        .map_err(|error| format!("Could not display confirmation: {error}"))?;
    wait_for_enter(terminal, true)?;
    for mut command in commands {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Err("Canceled. No further update commands were run.".into());
        }
        let status = command
            .status()
            .map_err(|error| format!("Could not execute {:?}: {error}", command.get_program()))?;
        if !status.success() || INTERRUPTED.load(Ordering::Relaxed) {
            if status.signal() == Some(libc::SIGINT)
                || status.code() == Some(130)
                || INTERRUPTED.load(Ordering::Relaxed)
            {
                return Err("Canceled. No further update commands were run.".into());
            }
            return Err(format!(
                "{:?} finished with {status}. No further update commands were run.",
                command.get_program()
            ));
        }
    }
    Ok(())
}

fn update_command(
    step: &UpdateCommand,
    program: &Path,
    sudo: Option<&Path>,
) -> Result<Command, String> {
    let mut command = if step.sudo {
        let mut command =
            Command::new(sudo.ok_or("This update command needs sudo, which is unavailable.")?);
        command.arg("--").arg(program);
        command
    } else {
        Command::new(program)
    };
    command
        .args(&step.args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // The runner catches Ctrl+C so it can display cancellation and hold the window.
    // SAFETY: signal is async-signal-safe; only the child's disposition is changed.
    unsafe {
        command.pre_exec(|| {
            if libc::signal(libc::SIGINT, libc::SIG_DFL) == libc::SIG_ERR {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command)
}

fn wait_for_enter(terminal: &mut File, cancelable: bool) -> Result<(), String> {
    terminal
        .flush()
        .map_err(|error| format!("Could not flush terminal output: {error}"))?;
    let mut byte = [0];
    loop {
        if cancelable && INTERRUPTED.load(Ordering::Relaxed) {
            return Err("Canceled before starting updates.".into());
        }
        match terminal.read(&mut byte) {
            Ok(0) => return Err("The controlling terminal closed.".into()),
            Ok(_) if byte[0] == b'\n' => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("Could not read the controlling terminal: {error}")),
        }
    }
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupt_handler(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

struct InterruptGuard(libc::sigaction);

impl InterruptGuard {
    fn install() -> Result<Self, String> {
        // SAFETY: sigaction is a C POD structure; zero initialization and sigemptyset
        // initialize the complete action before it is passed to the kernel.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            let mut previous: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = interrupt_handler as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            // No SA_RESTART: a blocked confirmation read must observe cancellation.
            if libc::sigaction(libc::SIGINT, &action, &mut previous) != 0 {
                return Err(format!(
                    "Could not install terminal cancellation handler: {}",
                    io::Error::last_os_error()
                ));
            }
            INTERRUPTED.store(false, Ordering::Relaxed);
            Ok(Self(previous))
        }
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        // SAFETY: restore the complete disposition saved by sigaction.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.0, std::ptr::null_mut());
        }
    }
}

struct RunnerLock {
    _file: File,
}

impl RunnerLock {
    fn acquire() -> Result<Self, String> {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")));
        Self::acquire_in(&runtime)
    }

    fn acquire_in(runtime: &Path) -> Result<Self, String> {
        // A private, user-owned runtime directory prevents lock-file replacement.
        let metadata = fs::symlink_metadata(runtime)
            .map_err(|error| format!("Could not inspect the user runtime directory: {error}"))?;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if !runtime.is_absolute()
            || !metadata.is_dir()
            || metadata.uid() != uid
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("Update locking requires a private, user-owned XDG_RUNTIME_DIR.".into());
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(runtime.join("cosmic-applet-system-updates.lock"))
            .map_err(|error| format!("Could not open the update lock: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("Could not inspect the update lock: {error}"))?;
        if !metadata.is_file()
            || metadata.uid() != uid
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("The update lock is not a private, user-owned regular file.".into());
        }
        // SAFETY: flock operates on the live file descriptor without taking pointers.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Err("Another System Updates terminal is already running. Finish or cancel it first.".into());
            }
            return Err(format!("Could not lock the update runner: {error}"));
        }
        // Do not unlink: every instance must lock the same inode. Closing releases flock.
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::symlink, sync::atomic::AtomicUsize};

    #[test]
    fn terminal_arguments_preserve_executable_path_without_shell_interpolation() {
        let executable = Path::new("/home/user/a path/'$(touch unwanted)'/updates");
        for &(_, prefix) in TERMINALS {
            let args = terminal_args(prefix, executable, Backend::Dnf5);
            assert_eq!(
                &args[prefix.len()..],
                &[
                    executable.as_os_str().to_owned(),
                    OsString::from("--run-updates"),
                    OsString::from("dnf5"),
                ]
            );
        }
        assert_eq!(TERMINALS.first().map(|entry| entry.0), Some("cosmic-term"));
    }

    #[test]
    fn sudo_arguments_remain_separate_and_literal() {
        let step = UpdateCommand {
            program: "dnf5".into(),
            args: vec!["upgrade".into(), "$(touch unwanted); 'quoted'".into()],
            sudo: true,
        };
        let command = update_command(
            &step,
            Path::new("/a path/dnf5"),
            Some(Path::new("/usr/bin/sudo")),
        )
        .unwrap();
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/sudo"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new("--"),
                OsStr::new("/a path/dnf5"),
                OsStr::new("upgrade"),
                OsStr::new("$(touch unwanted); 'quoted'"),
            ]
        );
    }

    #[test]
    fn aur_helpers_are_not_elevated() {
        let step = UpdateCommand {
            program: "paru".into(),
            args: vec!["-Syu".into()],
            sudo: false,
        };
        let command = update_command(&step, Path::new("/usr/bin/paru"), None).unwrap();
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/paru"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new("-Syu")]
        );
    }

    #[test]
    fn privileged_commands_reject_missing_sudo() {
        let step = UpdateCommand {
            program: "dnf5".into(),
            args: vec!["upgrade".into()],
            sudo: true,
        };
        assert!(update_command(&step, Path::new("/usr/bin/dnf5"), None).is_err());
    }

    static DIRECTORY_ID: AtomicUsize = AtomicUsize::new(0);

    fn runtime_directory() -> PathBuf {
        let directory = env::temp_dir().join(format!(
            "cosmic-updates-lock-test-{}-{}",
            std::process::id(),
            DIRECTORY_ID.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }

    #[test]
    fn flock_prevents_second_runner_and_releases_on_drop() {
        let directory = runtime_directory();
        let first = RunnerLock::acquire_in(&directory).unwrap();
        assert!(RunnerLock::acquire_in(&directory).is_err());
        drop(first);
        let second = RunnerLock::acquire_in(&directory).unwrap();
        drop(second);
        fs::remove_file(directory.join("cosmic-applet-system-updates.lock")).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn lock_rejects_symlinks_without_touching_the_target() {
        let directory = runtime_directory();
        let target = directory.join("target");
        fs::write(&target, "unchanged").unwrap();
        symlink(&target, directory.join("cosmic-applet-system-updates.lock")).unwrap();
        assert!(RunnerLock::acquire_in(&directory).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "unchanged");
        fs::remove_file(directory.join("cosmic-applet-system-updates.lock")).unwrap();
        fs::remove_file(target).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn lock_rejects_shared_runtime_directory() {
        let directory = runtime_directory();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(RunnerLock::acquire_in(&directory).is_err());
        fs::remove_dir(directory).unwrap();
    }
}
