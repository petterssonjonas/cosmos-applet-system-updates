// SPDX-License-Identifier: MPL-2.0

use crate::model::{
    Backend, Distribution, PackageUpdate, System, UpdateCommand, UpdatePlan, UpdateReport,
};
use std::{
    collections::HashMap,
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

const OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;
const CHECK_TIMEOUT: Duration = Duration::from_secs(300);

/// Detect the host distribution, never choosing a manager merely because it exists.
pub fn detect() -> Result<System, String> {
    let text = match fs::read_to_string("/etc/os-release") {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::read_to_string("/usr/lib/os-release")
                .map_err(|e| format!("Cannot read os-release: {e}"))?
        }
        Err(error) => return Err(format!("Cannot read /etc/os-release: {error}")),
    };
    let values = parse_os_release(&text)?;
    let id = values.get("ID").cloned().unwrap_or_else(|| "linux".into());
    let distro = Distribution {
        name: values
            .get("PRETTY_NAME")
            .or_else(|| values.get("NAME"))
            .cloned()
            .unwrap_or_else(|| id.clone()),
        id,
        id_like: values
            .get("ID_LIKE")
            .map(|v| v.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default(),
    };
    let atomic = Path::new("/run/ostree-booted").exists();
    let immutable = read_only("/")?
        || read_only("/usr")?
        || Path::new("/etc/transactional-update.conf").exists();
    Ok(select_system(
        distro,
        atomic,
        immutable,
        executable_available,
    ))
}

fn select_system(
    distro: Distribution,
    atomic: bool,
    immutable: bool,
    has: impl Fn(&str) -> bool,
) -> System {
    let ids = std::iter::once(distro.id.as_str()).chain(distro.id_like.iter().map(String::as_str));
    let unsupported = if distro.id == "steamos" || distro.id_like.iter().any(|id| id == "steamos") {
        Some(
            "SteamOS is a managed, read-only system. Use SteamOS Settings to update it; mutable pacman upgrades are not supported.",
        )
    } else if atomic {
        if has("rpm-ostree") {
            return System {
                distro,
                backend: Some(Backend::RpmOstree),
                detail: Some("Atomic deployments require a reboot after updating.".into()),
            };
        }
        Some(
            "This is an OSTree system, but rpm-ostree is unavailable. Use the distribution's image update tool.",
        )
    } else if immutable
        || matches!(
            distro.id.as_str(),
            "nixos"
                | "guix"
                | "microos"
                | "opensuse-microos"
                | "aeon"
                | "opensuse-aeon"
                | "kalpa"
                | "opensuse-kalpa"
                | "flatcar"
                | "talos"
        )
    {
        Some(
            "This read-only or declaratively managed system is unsupported. Use the distribution's transactional or image update tool.",
        )
    } else {
        None
    };
    if let Some(message) = unsupported {
        return System {
            distro,
            backend: None,
            detail: Some(message.into()),
        };
    }
    let mut candidates: &[(&str, Backend)] = &[];
    for id in ids {
        candidates = match id {
            "fedora" | "rhel" | "centos" | "rocky" | "almalinux" | "ol" | "amzn" | "nobara"
            | "ultramarine" => &[
                ("dnf5", Backend::Dnf5),
                ("dnf", Backend::Dnf),
                ("yum", Backend::Yum),
            ],
            "debian" | "ubuntu" | "pop" | "linuxmint" | "mint" | "elementary" | "kali"
            | "zorin" | "neon" => &[("apt-get", Backend::Apt)],
            "arch" | "manjaro" | "endeavouros" | "garuda" | "cachyos" | "artix" => &[
                ("paru", Backend::Paru),
                ("yay", Backend::Yay),
                ("pacman", Backend::Pacman),
            ],
            "opensuse" | "opensuse-tumbleweed" | "opensuse-leap" | "suse" | "sles" => {
                &[("zypper", Backend::Zypper)]
            }
            "alpine" => &[("apk", Backend::Apk)],
            "void" => &[("xbps-install", Backend::Xbps)],
            "solus" => &[("eopkg", Backend::Eopkg)],
            _ => continue,
        };
        break;
    }
    let backend = candidates
        .iter()
        .find(|(program, _)| has(program))
        .map(|(_, backend)| *backend);
    let detail = if backend.is_some() {
        None
    } else if candidates.is_empty() {
        Some(format!(
            "Unsupported distribution: {}. No supported host package manager is selected.",
            distro.name
        ))
    } else {
        Some(format!(
            "{} is recognized, but its package manager is missing (expected {}).",
            distro.name,
            candidates
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(" or ")
        ))
    };
    System {
        distro,
        backend,
        detail,
    }
}

fn read_only(path: &str) -> Result<bool, String> {
    let path = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated and stat points to writable storage.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(format!(
            "Cannot inspect host filesystem: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: statvfs initialized stat on success.
    Ok(unsafe { stat.assume_init() }.f_flag & libc::ST_RDONLY != 0)
}

fn parse_os_release(text: &str) -> Result<HashMap<String, String>, String> {
    let mut values = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, raw) = line
            .split_once('=')
            .ok_or_else(|| format!("Invalid os-release assignment on line {}", index + 1))?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        {
            return Err(format!("Invalid os-release key on line {}", index + 1));
        }
        let value = release_value(raw.trim())
            .map_err(|e| format!("Invalid os-release line {}: {e}", index + 1))?;
        if value.chars().any(char::is_control) {
            return Err("os-release contains control characters".into());
        }
        values.insert(key.into(), value);
    }
    Ok(values)
}

// os-release has shell-like quoting, but expansion and execution are never allowed.
fn release_value(raw: &str) -> Result<String, String> {
    let quote = raw.chars().next().filter(|c| *c == '\'' || *c == '"');
    let body = if let Some(quote) = quote {
        if raw.len() < 2 || !raw.ends_with(quote) {
            return Err("unterminated quote".into());
        }
        &raw[1..raw.len() - 1]
    } else {
        raw
    };
    let mut result = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if Some(c) == quote {
            return Err("concatenated quotes are not supported by os-release".into());
        }
        if c == '\\' && quote != Some('\'') {
            let next = chars.next().ok_or("trailing escape")?;
            if quote == Some('"') && !matches!(next, '$' | '`' | '"' | '\\') {
                result.push('\\');
            }
            result.push(next);
        } else {
            if quote.is_none() && (c.is_whitespace() || matches!(c, '\'' | '"')) {
                return Err("unquoted whitespace or quote".into());
            }
            result.push(c);
        }
    }
    Ok(result)
}

fn executable_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|candidate| {
                fs::metadata(candidate)
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    })
}
fn executable_available(name: &str) -> bool {
    executable_path(name).is_some()
}

fn require(program: &str, message: &str) -> Result<(), String> {
    if executable_available(program) {
        Ok(())
    } else {
        Err(format!("Fresh update check requires {program}. {message}"))
    }
}

fn selected(system: &System) -> Result<Backend, String> {
    system.backend.ok_or_else(|| {
        system
            .detail
            .clone()
            .unwrap_or_else(|| "Unsupported host update backend".into())
    })
}

/// Refresh metadata and query updates without executing an upgrade or requesting root.
pub async fn check(system: &System) -> Result<UpdateReport, String> {
    let backend = selected(system)?;
    let mut report = match backend {
        Backend::Dnf5 | Backend::Dnf | Backend::Yum => check_dnf(backend).await?,
        Backend::Apt => check_apt().await?,
        Backend::Pacman | Backend::Paru | Backend::Yay => check_arch(backend).await?,
        Backend::Zypper => check_zypper().await?,
        Backend::RpmOstree => check_atomic().await?,
        Backend::Apk => check_apk().await?,
        Backend::Xbps => {
            // XBPS's documented memory-sync ignores disk indexes and never changes state with -n.
            let output = run("xbps-install", &["-M", "-n", "-u"], &[], &[0]).await?;
            UpdateReport {
                packages: parse_xbps(&output.stdout)?,
                note: None,
            }
        }
        Backend::Eopkg => check_eopkg().await?,
    };
    report.packages.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.available.cmp(&b.available))
            .then(a.source.cmp(&b.source))
    });
    report.packages.dedup();
    Ok(report)
}

/// Return fixed, interactive upgrade commands. This function never executes them.
pub fn update_plan(system: &System) -> Result<UpdatePlan, String> {
    let backend = selected(system)?;
    let step = |program: &str, args: &[&str], sudo| UpdateCommand {
        program: program.into(),
        args: args.iter().map(|s| (*s).into()).collect(),
        sudo,
    };
    let steps = match backend {
        Backend::Dnf5 | Backend::Dnf => vec![step(backend.id(), &["upgrade", "--refresh"], true)],
        Backend::Yum => vec![step(
            "yum",
            &["--setopt=*.metadata_expire=0", "upgrade"],
            true,
        )],
        Backend::Apt => vec![
            step("apt-get", &["update", "--error-on=any"], true),
            step("apt-get", &["dist-upgrade"], true),
        ],
        Backend::Pacman => vec![step("pacman", &["-Syu"], true)],
        Backend::Paru | Backend::Yay => vec![step(backend.id(), &["-Syu"], false)],
        Backend::Zypper => vec![
            step("zypper", &["refresh"], true),
            step(
                "zypper",
                &[
                    if system.distro.id.contains("tumbleweed")
                        || system
                            .distro
                            .id_like
                            .iter()
                            .any(|id| id.contains("tumbleweed"))
                    {
                        "dup"
                    } else {
                        "update"
                    },
                ],
                true,
            ),
        ],
        Backend::RpmOstree => vec![step("rpm-ostree", &["upgrade"], true)],
        Backend::Apk => vec![
            step("apk", &["update"], true),
            step("apk", &["--interactive", "upgrade"], true),
        ],
        Backend::Xbps => vec![step("xbps-install", &["-S", "-u"], true)],
        Backend::Eopkg => vec![
            step("eopkg", &["update-repo"], true),
            step("eopkg", &["upgrade"], true),
        ],
    };
    Ok(UpdatePlan { steps })
}

struct Captured {
    code: i32,
    stdout: String,
    stderr: String,
}

struct ProcessGroup(Option<u32>);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // SAFETY: the child creates its own process group. Negative pid targets only that group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

async fn bounded_output(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| format!("Cannot read package manager output: {e}"))?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        Err("Package manager output exceeded the 8 MiB safety limit".into())
    } else {
        Ok(bytes)
    }
}

async fn run(
    program: &str,
    args: &[&str],
    environment: &[(&str, &Path)],
    accepted: &[i32],
) -> Result<Captured, String> {
    let executable = executable_path(program).ok_or_else(|| format!("Fresh update check requires {program}. Install the distribution's official package manager or repair PATH."))?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    for (key, value) in environment {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start {program}: {e}"))?;
    let mut group = ProcessGroup(child.id());
    let stdout = child.stdout.take().ok_or("Cannot capture stdout")?;
    let stderr = child.stderr.take().ok_or("Cannot capture stderr")?;
    let mut out_task = tokio::spawn(bounded_output(stdout));
    let mut err_task = tokio::spawn(bounded_output(stderr));
    let collected = tokio::time::timeout(CHECK_TIMEOUT, async {
        let status = child
            .wait()
            .await
            .map_err(|e| format!("Cannot wait for {program}: {e}"))?;
        let out = (&mut out_task).await.map_err(|e| e.to_string())??;
        let err = (&mut err_task).await.map_err(|e| e.to_string())??;
        Ok::<_, String>((status, out, err))
    })
    .await;
    let (status, stdout, stderr) = match collected {
        Ok(Ok(value)) => {
            group.0 = None;
            value
        }
        result => {
            drop(group);
            let _ = child.kill().await;
            out_task.abort();
            err_task.abort();
            return Err(match result {
                Ok(Err(error)) => error,
                _ => format!("{program} timed out after 300 seconds; the check was terminated"),
            });
        }
    };
    let output = Captured {
        code: status_code(status)?,
        stdout: String::from_utf8(stdout)
            .map_err(|_| format!("{program} returned invalid UTF-8"))?,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    };
    validate_status(program, &output, accepted)?;
    Ok(output)
}

fn status_code(status: ExitStatus) -> Result<i32, String> {
    status
        .code()
        .ok_or_else(|| "Package manager terminated by a signal".into())
}

fn validate_status(program: &str, output: &Captured, accepted: &[i32]) -> Result<(), String> {
    if !accepted.contains(&output.code)
        || metadata_error(&output.stderr)
        || metadata_error(&output.stdout)
    {
        let detail = if output.stderr.trim().is_empty() {
            &output.stdout
        } else {
            &output.stderr
        };
        return Err(format!(
            "{program} check failed (exit {}): {}",
            output.code,
            display_text(detail, 1200)
        ));
    }
    Ok(())
}

fn metadata_error(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim().to_ascii_lowercase();
        line.starts_with("error:")
            || line.starts_with("e:")
            || line.starts_with("err:")
            || line.starts_with("failed to ")
            || line.contains("failed to download")
            || line.contains("some index files failed")
            || line.contains("ignoring repositories")
            || line.contains("no active repositories")
            || line.contains("no enabled repositories")
            || line.contains("no repositories defined")
    })
}

fn display_text(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}

struct PrivateDirectory(PathBuf);
impl PrivateDirectory {
    fn new() -> Result<Self, String> {
        let template = std::env::temp_dir().join("cosmic-system-updates-XXXXXX");
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let mut bytes = template.as_os_str().as_bytes().to_vec();
        bytes.push(0);
        // SAFETY: mkdtemp receives writable, NUL-terminated storage containing six trailing Xs.
        if unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) }.is_null() {
            return Err(format!(
                "Cannot create private update cache: {}",
                io::Error::last_os_error()
            ));
        }
        bytes.pop();
        Ok(Self(PathBuf::from(std::ffi::OsString::from_vec(bytes))))
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// The lock stays held across metadata refresh and the subsequent query. Native
// managers retain their conditional-download state, but host configuration is read anew.
struct PersistentCache {
    path: PathBuf,
    _lock: fs::File,
    _directory: fs::File,
}

impl PersistentCache {
    fn new(backend: &str) -> Result<Self, String> {
        let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from).filter(|path| path.is_absolute()).map(|home| home.join(".cache")))
            .ok_or("Fresh checks require an absolute XDG_CACHE_HOME or HOME for private metadata caches")?;
        Self::at(&base, backend)
    }

    fn at(base: &Path, backend: &str) -> Result<Self, String> {
        if !base.is_absolute()
            || !backend
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            || backend.is_empty()
        {
            return Err("Invalid update cache location".into());
        }
        // SAFETY: geteuid only queries the current process identity.
        let uid = unsafe { libc::geteuid() };
        let mut directory = fs::File::open("/").map_err(|e| e.to_string())?;
        for component in base.components() {
            match component {
                Component::RootDir => continue,
                Component::Normal(name) => {
                    directory = cache_directory_at(&directory, name, false, uid)?;
                }
                _ => {
                    return Err("Update cache paths must not contain '.' or '..' components".into());
                }
            }
        }
        if directory.metadata().map_err(|e| e.to_string())?.uid() != uid {
            return Err("The update cache base must belong to the current user".into());
        }
        directory = cache_directory_at(
            &directory,
            std::ffi::OsStr::new("cosmic-applet-system-updates"),
            true,
            uid,
        )?;
        directory = cache_directory_at(&directory, std::ffi::OsStr::new(backend), true, uid)?;
        let lock_name = c".lock";
        // SAFETY: directory is a live directory descriptor; name is NUL-terminated.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                lock_name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(format!(
                "Cannot open private update cache lock: {}",
                io::Error::last_os_error()
            ));
        }
        // SAFETY: openat returned a new descriptor, now owned solely by this File.
        let lock = unsafe { fs::File::from_raw_fd(fd) };
        let metadata = lock.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.uid() != uid
            || metadata.nlink() != 1
            || metadata.mode() & 0o777 != 0o600
        {
            return Err("Unsafe update cache lock: expected an owned, singly linked regular file with mode 0600".into());
        }
        // SAFETY: flock operates on the valid descriptor retained for the entire check.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            return Err(if error.kind() == io::ErrorKind::WouldBlock {
                format!(
                    "Another {backend} update check is using the metadata cache; try again after it finishes"
                )
            } else {
                format!("Cannot lock the update cache: {error}")
            });
        }
        Ok(Self {
            path: base.join("cosmic-applet-system-updates").join(backend),
            _lock: lock,
            _directory: directory,
        })
    }
}

fn cache_directory_at(
    parent: &fs::File,
    name: &std::ffi::OsStr,
    private: bool,
    uid: u32,
) -> Result<fs::File, String> {
    let name = std::ffi::CString::new(name.as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: parent is a live directory descriptor and name is NUL-terminated.
    let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
    if created != 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
        return Err(format!(
            "Cannot create update cache directory: {}",
            io::Error::last_os_error()
        ));
    }
    // O_NOFOLLOW protects every path component, not merely the final cache directory.
    // SAFETY: parent is live and name is NUL-terminated; no mutable memory is shared.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!(
            "Unsafe or unreadable update cache path: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: the successful openat descriptor is transferred exactly once.
    let directory = unsafe { fs::File::from_raw_fd(fd) };
    let metadata = directory.metadata().map_err(|e| e.to_string())?;
    let mode = metadata.mode() & 0o7777;
    let trusted_ancestor = (metadata.uid() == uid || metadata.uid() == 0)
        && (mode & 0o022 == 0 || metadata.uid() == 0 && mode & 0o1000 != 0);
    if !metadata.is_dir()
        || !trusted_ancestor
        || private && (metadata.uid() != uid || mode != 0o700)
    {
        return Err("Unsafe update cache permissions: application directories must be owned by this user with mode 0700, and ancestors must not be writable by other users".into());
    }
    Ok(directory)
}

async fn check_dnf(backend: Backend) -> Result<UpdateReport, String> {
    // Coordinate applet/CLI checks, but retain DNF's configured user cache and
    // existing per-repository trust rather than creating a second keyring.
    let _coordination = PersistentCache::new(backend.id())?;
    let refresh = if backend == Backend::Yum {
        "--setopt=*.metadata_expire=0"
    } else {
        "--refresh"
    };
    let mut args = vec![
        "--quiet",
        "--color=never",
        refresh,
        "--setopt=assumeyes=False",
        "--setopt=assumeno=False",
        "--setopt=defaultyes=False",
        "--setopt=*.skip_if_unavailable=False",
        "check-update",
    ];
    if backend == Backend::Dnf5 {
        args.push("--json");
    }
    let output = run(backend.id(), &args, &[], &[0, 100])
        .await
        .map_err(|error| dnf_check_error(backend, error))?;
    let mut packages = if backend == Backend::Dnf5 {
        parse_dnf5(&output.stdout)?
    } else {
        parse_dnf(&output.stdout)
    };
    if output.code == 100 && packages.is_empty() {
        return Err("DNF reported updates, but its package output could not be parsed".into());
    }
    if output.code == 0 && !packages.is_empty() {
        return Err("DNF returned an inconsistent update status".into());
    }
    if !packages.is_empty() {
        let installed = run(
            "rpm",
            &[
                "-qa",
                "--qf",
                "%{NAME}.%{ARCH}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}\n",
            ],
            &[],
            &[0],
        )
        .await?;
        let versions: HashMap<_, _> = installed
            .stdout
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect();
        for package in &mut packages {
            package.installed = versions
                .get(package.name.as_str())
                .map(|value| (*value).into());
        }
    }
    Ok(UpdateReport {
        packages,
        note: None,
    })
}

fn dnf_check_error(backend: Backend, error: String) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("openpgp") || lower.contains("gpg key") || lower.contains("public key") {
        let refresh = if backend == Backend::Yum {
            "--setopt=*.metadata_expire=0"
        } else {
            "--refresh"
        };
        format!(
            "{error}\nRepository signing keys need manual review in DNF's normal user cache. Run '{} {refresh} check-update' as your normal user in a terminal, verify the fingerprints, and approve only trusted keys. This checks metadata; it does not upgrade packages. Background checks never approve new keys.",
            backend.id()
        )
    } else {
        error
    }
}

fn package_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'+' | b'.' | b':'))
}
fn version(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().any(|c| c.is_ascii_digit())
        && !value.chars().any(char::is_control)
}
fn package(
    name: &str,
    installed: Option<&str>,
    available: &str,
    source: Option<&str>,
) -> PackageUpdate {
    PackageUpdate {
        name: name.into(),
        installed: installed.map(str::to_owned),
        available: available.into(),
        source: source.map(str::to_owned),
    }
}
fn parse_dnf(text: &str) -> Vec<PackageUpdate> {
    text.lines()
        .filter_map(|line| {
            let columns: Vec<_> = line.split_whitespace().collect();
            if columns.len() != 3
                || !package_name(columns[0])
                || !columns[0].contains('.')
                || !version(columns[1])
            {
                return None;
            }
            Some(package(columns[0], None, columns[1], Some(columns[2])))
        })
        .collect()
}

fn parse_dnf5(text: &str) -> Result<Vec<PackageUpdate>, String> {
    let sections: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("Invalid DNF5 update JSON: {e}"))?;
    let sections = sections
        .as_object()
        .ok_or("DNF5 update JSON is not an object")?;
    let mut packages = Vec::new();
    for rows in sections.values() {
        for row in rows
            .as_array()
            .ok_or("DNF5 update section is not an array")?
        {
            let field = |key| {
                row.get(key)
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| format!("DNF5 update row has no {key}"))
            };
            let name = field("name")?;
            let arch = field("arch")?;
            let available = field("evr")?;
            let source = field("repository")?;
            if !package_name(name) || !package_name(arch) || !version(available) {
                return Err("Invalid DNF5 package data".into());
            }
            packages.push(package(
                &format!("{name}.{arch}"),
                None,
                available,
                Some(source),
            ));
        }
    }
    Ok(packages)
}

async fn check_apt() -> Result<UpdateReport, String> {
    require(
        "apt",
        "APT's list command is needed to report all upgradable packages.",
    )?;
    let persistent = PersistentCache::new("apt")?;
    fs::create_dir_all(persistent.path.join("lists/partial")).map_err(|e| e.to_string())?;
    let private = PrivateDirectory::new()?;
    let config = private.0.join("apt.conf");
    // APT runs local administrator hooks even for 'update'; do not run system hooks in a private scan.
    fs::write(&config, "#clear APT::Update::Pre-Invoke;\n#clear APT::Update::Post-Invoke;\n#clear APT::Update::Post-Invoke-Success;\n").map_err(|e| e.to_string())?;
    let config = config.to_str().ok_or("APT cache path is not UTF-8")?;
    let lists = format!("Dir::State::lists={}/lists", persistent.path.display());
    let cache = format!("Dir::Cache={}", persistent.path.display());
    let mut args = vec![
        "-c",
        config,
        "-o",
        &lists,
        "-o",
        &cache,
        "-o",
        "Dir::Cache::pkgcache=",
        "-o",
        "Dir::Cache::srcpkgcache=",
        "-o",
        "Acquire::AllowInsecureRepositories=false",
        "-o",
        "Acquire::AllowDowngradeToInsecureRepositories=false",
        "-o",
        "APT::Get::AllowUnauthenticated=false",
    ];
    args.extend(["update", "--error-on=any"]);
    run("apt-get", &args, &[], &[0]).await?;
    args.truncate(args.len() - 2);
    args.extend(["indextargets", "--format", "$(IDENTIFIER)"]);
    let targets = run("apt-get", &args, &[], &[0]).await?;
    if !targets.stdout.lines().any(|line| line.trim() == "Packages") {
        return Err("APT refreshed no binary package indices. Configure an enabled binary repository before checking updates.".into());
    }
    args.truncate(args.len() - 3);
    args.extend(["list", "--upgradable"]);
    let output = run("apt", &args, &[], &[0]).await?;
    Ok(UpdateReport {
        packages: parse_apt(&output.stdout)?,
        note: None,
    })
}

fn parse_apt(text: &str) -> Result<Vec<PackageUpdate>, String> {
    let mut packages = Vec::new();
    for line in text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with("Listing"))
    {
        let columns: Vec<_> = line.split_whitespace().collect();
        let Some((name, source)) = columns.first().and_then(|c| c.split_once('/')) else {
            return Err(format!(
                "Unrecognized APT update output: {}",
                display_text(line, 160)
            ));
        };
        if columns.len() < 3 || !package_name(name) || !version(columns[1]) {
            return Err("Invalid APT package output".into());
        }
        let installed = line
            .split_once("[upgradable from: ")
            .and_then(|(_, rest)| rest.strip_suffix(']'));
        packages.push(package(name, installed, columns[1], Some(source)));
    }
    Ok(packages)
}

async fn check_arch(backend: Backend) -> Result<UpdateReport, String> {
    require(
        "checkupdates",
        "Install pacman-contrib manually. Never refresh the live pacman database just to check updates.",
    )?;
    require(
        "fakeroot",
        "The checkupdates helper needs fakeroot for its private database.",
    )?;
    require("pacman-conf", "The checkupdates helper needs pacman-conf.")?;
    // All Arch helpers share the repository database, so they also share its lock.
    let persistent = PersistentCache::new("arch")?;
    let configuration = run("pacman-conf", &["DBPath"], &[], &[0]).await?;
    let database = Path::new(configuration.stdout.trim());
    if !database.is_absolute() || !database.join("local").is_dir() {
        return Err(
            "pacman-conf returned no readable absolute installed-package database path".into(),
        );
    }
    let target = database.join("local");
    let link = persistent.path.join("local");
    match fs::symlink_metadata(&link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(&link).map_err(|e| e.to_string())? != target {
                fs::remove_file(&link).map_err(|e| e.to_string())?;
                std::os::unix::fs::symlink(&target, &link).map_err(|e| e.to_string())?;
            }
        }
        Ok(_) => {
            return Err(
                "Unsafe Arch update cache: 'local' must be a link to the host package database"
                    .into(),
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::os::unix::fs::symlink(&target, &link).map_err(|e| e.to_string())?;
        }
        Err(error) => return Err(format!("Cannot inspect Arch update cache: {error}")),
    }
    let output = run(
        "checkupdates",
        &["--nocolor"],
        &[("CHECKUPDATES_DB", &persistent.path)],
        &[0, 2],
    )
    .await?;
    let mut packages = parse_arrows(&output.stdout, "repositories")?;
    if output.code == 2 && !packages.is_empty() {
        return Err("checkupdates returned empty status with package updates".into());
    }
    if matches!(backend, Backend::Paru | Backend::Yay) {
        let output = run(backend.id(), &["-Qua", "--color", "never"], &[], &[0, 1]).await?;
        // pacman-style queries return 1 for an empty result; errors remain errors even with exit 1.
        if output.code == 1
            && (!output.stdout.trim().is_empty() || !output.stderr.trim().is_empty())
        {
            return Err(format!(
                "AUR query failed: {}",
                display_text(&output.stderr, 800)
            ));
        }
        packages.extend(parse_arrows(&output.stdout, "AUR")?);
    }
    Ok(UpdateReport { packages, note: Some("Repository metadata was refreshed in a private checkupdates database. AUR checks use the selected helper; development commits depend on helper configuration.".into()) })
}

fn parse_arrows(text: &str, source: &str) -> Result<Vec<PackageUpdate>, String> {
    let mut packages = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let columns: Vec<_> = line.split_whitespace().collect();
        if columns.len() < 4
            || columns[2] != "->"
            || !package_name(columns[0])
            || !version(columns[1])
            || !(version(columns[3]) || columns[3] == "latest-commit")
        {
            return Err(format!(
                "Unrecognized update query output: {}",
                display_text(line, 160)
            ));
        }
        packages.push(package(
            columns[0],
            Some(columns[1]),
            columns[3],
            Some(source),
        ));
    }
    Ok(packages)
}

// Copy only manager state/configuration, not packages. Symlinked files are read, never recreated.
fn copy_tree(source: &Path, target: &Path, remaining: &mut u64) -> Result<(), String> {
    // The host's cache symlink is irrelevant: APK is explicitly given a private cache path.
    if source == Path::new("/etc/apk/cache") {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(source)
        .map_err(|e| format!("Cannot read {}: {e}", source.display()))?;
    if metadata.is_dir() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(target)
            .map_err(|e| e.to_string())?;
        for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            copy_tree(&entry.path(), &target.join(entry.file_name()), remaining)?;
        }
    } else {
        let metadata = fs::metadata(source).map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err(format!(
                "Cannot snapshot non-regular manager state {}",
                source.display()
            ));
        }
        *remaining = remaining
            .checked_sub(metadata.len())
            .ok_or("Manager state exceeds the 512 MiB snapshot limit")?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(source, target)
            .map_err(|e| format!("Cannot snapshot {}: {e}", source.display()))?;
        fs::set_permissions(target, fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn snapshot(private: &Path, paths: &[&str]) -> Result<(), String> {
    let root = private.to_owned();
    let paths: Vec<_> = paths.iter().map(|path| (*path).to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        let mut remaining = 512 * 1024 * 1024;
        for path in paths {
            copy_tree(
                Path::new(&path),
                &root.join(path.trim_start_matches('/')),
                &mut remaining,
            )?;
        }
        Ok::<_, String>(())
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn check_zypper() -> Result<UpdateReport, String> {
    let private = PrivateDirectory::new()?;
    let database = ["/usr/lib/sysimage/rpm", "/var/lib/rpm"]
        .into_iter()
        .find(|path| {
            Path::new(path).join("rpmdb.sqlite").exists()
                || Path::new(path).join("Packages").exists()
        })
        .ok_or("Fresh Zypper checks require a readable host RPM database to snapshot")?;
    let mut paths = vec![database, "/etc/zypp", "/etc/os-release"];
    if Path::new("/etc/products.d").exists() {
        paths.push("/etc/products.d");
    }
    snapshot(&private.0, &paths).await?;
    // NeedsRootCondition permits --root: all cache/key/database writes target this private snapshot.
    let root = private.0.to_str().ok_or("Zypper cache path is not UTF-8")?;
    run(
        "zypper",
        &["--root", root, "--non-interactive", "refresh", "--force"],
        &[],
        &[0],
    )
    .await?;
    let output = run(
        "zypper",
        &[
            "--root",
            root,
            "--non-interactive",
            "--no-refresh",
            "list-updates",
            "--all",
        ],
        &[],
        &[0],
    )
    .await?;
    Ok(UpdateReport { packages: parse_zypper(&output.stdout)?, note: Some("Fresh repository metadata checked against a private snapshot of the host RPM database.".into()) })
}

fn parse_zypper(text: &str) -> Result<Vec<PackageUpdate>, String> {
    let mut packages = Vec::new();
    let mut header = None;
    for line in text.lines().filter(|line| line.contains('|')) {
        let columns: Vec<_> = line.split('|').map(str::trim).collect();
        if columns.contains(&"Available Version") {
            header = Some(columns);
            continue;
        }
        let Some(header) = &header else {
            continue;
        };
        let get = |name| {
            header
                .iter()
                .position(|column| *column == name)
                .and_then(|i| columns.get(i))
                .copied()
        };
        let (Some(name), Some(available), Some(installed), Some(source)) = (
            get("Name"),
            get("Available Version"),
            get("Current Version"),
            get("Repository"),
        ) else {
            return Err("Invalid Zypper package table".into());
        };
        if !package_name(name) || !version(available) {
            return Err("Invalid Zypper update row".into());
        }
        packages.push(package(name, Some(installed), available, Some(source)));
    }
    if header.is_none() && !text.contains("No updates found") {
        return Err("Zypper did not return an update table or an explicit empty result".into());
    }
    Ok(packages)
}

async fn check_apk() -> Result<UpdateReport, String> {
    let private = PrivateDirectory::new()?;
    snapshot(&private.0, &["/etc/apk", "/lib/apk/db/installed"]).await?;
    fs::create_dir_all(private.0.join("var/cache/apk")).map_err(|e| e.to_string())?;
    let root = private.0.to_str().ok_or("APK cache path is not UTF-8")?;
    let refreshed = run(
        "apk",
        &[
            "--root",
            root,
            "--cache-dir",
            "var/cache/apk",
            "--no-progress",
            "update",
        ],
        &[],
        &[0],
    )
    .await?;
    if refreshed.stdout.contains("0 distinct packages available") {
        return Err("APK refreshed no package indices. Configure an enabled Alpine repository before checking updates.".into());
    }
    let output = run(
        "apk",
        &[
            "--root",
            root,
            "--cache-dir",
            "var/cache/apk",
            "--no-network",
            "version",
            "--limit",
            "<",
        ],
        &[],
        &[0],
    )
    .await?;
    Ok(UpdateReport {
        packages: parse_apk(&output.stdout)?,
        note: None,
    })
}

fn split_pkgver(value: &str) -> Option<(&str, &str)> {
    value
        .char_indices()
        .rev()
        .find(|(index, c)| {
            *c == '-'
                && value
                    .as_bytes()
                    .get(index + 1)
                    .is_some_and(u8::is_ascii_digit)
        })
        .map(|(index, _)| (&value[..index], &value[index + 1..]))
}
fn parse_apk(text: &str) -> Result<Vec<PackageUpdate>, String> {
    let mut packages = Vec::new();
    for line in text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with("Installed:"))
    {
        let columns: Vec<_> = line.split_whitespace().collect();
        let Some((name, installed)) = columns.first().and_then(|value| split_pkgver(value)) else {
            return Err("Invalid APK package version output".into());
        };
        if columns.len() < 3 || columns[1] != "<" || !package_name(name) || !version(columns[2]) {
            return Err("Invalid APK update output".into());
        }
        packages.push(package(
            name,
            Some(installed),
            columns[2],
            columns.get(3).copied(),
        ));
    }
    Ok(packages)
}
fn parse_xbps(text: &str) -> Result<Vec<PackageUpdate>, String> {
    let mut packages = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let columns: Vec<_> = line.split_whitespace().collect();
        if columns.len() != 6
            || !matches!(
                columns[1],
                "update" | "install" | "remove" | "configure" | "reinstall" | "hold"
            )
        {
            return Err(format!(
                "Invalid XBPS dry-run output: {}",
                display_text(line, 160)
            ));
        }
        if columns[1] != "update" {
            continue;
        }
        let Some((name, available)) = split_pkgver(columns[0]) else {
            return Err("Invalid XBPS package version".into());
        };
        packages.push(package(name, None, available, Some(columns[3])));
    }
    Ok(packages)
}

async fn check_atomic() -> Result<UpdateReport, String> {
    require(
        "pkcheck",
        "Atomic background checks require polkit's noninteractive authorization probe.",
    )?;
    // rpm-ostreed allows interactive authorization even for --check. Do not invoke
    // it unless policy authorizes this user without relying on an expiring sudo-like grant.
    let grants = run("pkcheck", &["--list-temp"], &[], &[0]).await?;
    if !grants.stdout.trim().is_empty() {
        return Err("Atomic background checks need permanent noninteractive polkit authorization; temporary authentication grants are not used. Update manually in a terminal.".into());
    }
    let stat = fs::read_to_string("/proc/self/stat").map_err(|e| e.to_string())?;
    let start = stat
        .rsplit_once(')')
        .and_then(|(_, tail)| tail.split_whitespace().nth(19))
        .ok_or("Cannot identify the applet process for polkit")?;
    // SAFETY: geteuid takes no pointers and only queries the current identity.
    let subject = format!("{},{},{}", std::process::id(), start, unsafe {
        libc::geteuid()
    });
    let authorization = run(
        "pkcheck",
        &[
            "--action-id",
            "org.projectatomic.rpmostree1.upgrade",
            "--process",
            &subject,
        ],
        &[],
        &[0, 1, 2, 3],
    )
    .await?;
    if authorization.code != 0 {
        return Err("rpm-ostree needs noninteractive polkit permission to refresh metadata. Background checks will not request a password. Use the terminal updater or ask the administrator to authorize update checks.".into());
    }
    // --check downloads metadata only; no deployment is created.
    let output = run("rpm-ostree", &["upgrade", "--check"], &[], &[0, 77]).await?;
    if output.stdout.contains("Automatic updates are not enabled") {
        return Err(
            "rpm-ostree did not check updates: its automatic check policy is disabled".into(),
        );
    }
    let status = run("rpm-ostree", &["status", "--json"], &[], &[0]).await?;
    parse_atomic(&status.stdout, output.code)
}
fn parse_atomic(text: &str, code: i32) -> Result<UpdateReport, String> {
    let status: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("Invalid rpm-ostree deployment JSON: {e}"))?;
    let deployments = status
        .get("deployments")
        .and_then(serde_json::Value::as_array)
        .ok_or("rpm-ostree returned no deployments array")?;
    let booted = deployments
        .iter()
        .find(|deployment| {
            deployment
                .get("booted")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        })
        .ok_or("rpm-ostree returned no booted deployment")?;
    let mut packages = Vec::new();
    if code == 0 {
        let update = status
            .get("cached-update")
            .filter(|value| value.is_object())
            .ok_or("rpm-ostree reported an update but returned no fresh cached deployment data")?;
        let checksum = update
            .get("checksum")
            .and_then(serde_json::Value::as_str)
            .ok_or("rpm-ostree update has no deployment checksum")?;
        let available = update
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(checksum);
        let installed = booted
            .get("version")
            .or_else(|| booted.get("checksum"))
            .and_then(serde_json::Value::as_str);
        let origin = update
            .get("origin")
            .or_else(|| booted.get("origin"))
            .and_then(serde_json::Value::as_str);
        let name = update
            .get("osname")
            .or_else(|| booted.get("osname"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("OSTree deployment");
        packages.push(package(name, installed, available, origin));
    }
    let pending = deployments.first().is_some_and(|deployment| {
        deployment
            .get("booted")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    });
    Ok(UpdateReport { packages, note: Some(if pending { "A deployment is already pending; reboot to apply it. Available items describe real OS deployments, not individual packages." } else { "Available items describe real OS deployments, not individual packages. Updating stages a deployment; reboot to apply it." }.into()) })
}

// eopkg's CLI may invoke pkexec even with --destdir. Its own Python API lets us set a
// private root before any write, retaining native repository and version handling.
const EOPKG_CHECK: &str = r#"
import contextlib, json, os, shutil, sys
root = sys.argv[1]
with contextlib.redirect_stdout(sys.stderr):
    import pisi.api, pisi.cli, pisi.config, pisi.context as ctx, pisi.db
    import pisi.db.installdb, pisi.db.packagedb, pisi.db.repodb
    if ctx.config.dest_dir() != '/':
        raise RuntimeError('Custom eopkg system roots require a dedicated checker')
    paths = [ctx.config.info_dir(), ctx.config.packages_dir()]
    remaining = 512 * 1024 * 1024
    for source in dict.fromkeys(paths):
        if not os.path.isdir(source):
            raise RuntimeError('Missing eopkg installed/repository state: ' + source)
        for base, dirs, files in os.walk(source, followlinks=False):
            if any(os.path.islink(os.path.join(base, d)) for d in dirs):
                raise RuntimeError('Symlinked eopkg state directories are unsupported')
            for filename in files:
                path = os.path.join(base, filename)
                remaining -= os.stat(path).st_size
                if remaining < 0:
                    raise RuntimeError('eopkg state exceeds the 512 MiB snapshot limit')
        target = os.path.join(root, source.lstrip('/'))
        shutil.copytree(source, target, dirs_exist_ok=True)
    options = pisi.config.Options()
    options.destdir = root
    options.no_color = True
    options.retry_attempts = 0
    pisi.api.set_options(options)
    pisi.api.set_userinterface(pisi.cli.CLI())
    pisi.api.set_can_configure(False)
    if not os.path.isfile(os.path.join(ctx.config.info_dir(), ctx.const.repos)):
        raise RuntimeError('No configured eopkg repository state; no default repository will be added')
    pisi.db.invalidate_caches()
    repos = pisi.db.repodb.RepoDB().list_repos()
    if not repos:
        raise RuntimeError('No active eopkg repositories configured')
    pisi.api.update_repos(repos, force=True)
    if ctx.ui.errors:
        raise RuntimeError('eopkg metadata refresh reported errors')
    installed = pisi.db.installdb.InstallDB()
    available = pisi.db.packagedb.PackageDB()
    updates = []
    for name in pisi.api.list_upgradable():
        candidate, repo = available.get_package_repo(name)
        old = installed.get_package(name) if installed.has_package(name) else None
        updates.append({'name': name, 'installed': str(old.version) + '-' + str(old.release) if old else None,
            'available': str(candidate.version) + '-' + str(candidate.release), 'source': repo})
print(json.dumps(updates))
"#;

async fn check_eopkg() -> Result<UpdateReport, String> {
    require(
        "python3",
        "The eopkg Python 3 API (pisi) is needed for a safe, unprivileged fresh check. Repair or update the official eopkg installation manually.",
    )?;
    let private = PrivateDirectory::new()?;
    let root = private.0.to_str().ok_or("eopkg cache path is not UTF-8")?;
    let output = run("python3", &["-I", "-c", EOPKG_CHECK, root], &[], &[0]).await?;
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        installed: Option<String>,
        available: String,
        source: Option<String>,
    }
    let rows: Vec<Row> = serde_json::from_str(&output.stdout)
        .map_err(|e| format!("Invalid eopkg API output: {e}"))?;
    if rows
        .iter()
        .any(|row| !package_name(&row.name) || !version(&row.available))
    {
        return Err("Invalid package data from eopkg API".into());
    }
    Ok(UpdateReport {
        packages: rows
            .into_iter()
            .map(|row| PackageUpdate {
                name: row.name,
                installed: row.installed,
                available: row.available,
                source: row.source,
            })
            .collect(),
        note: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distro(id: &str, like: &[&str]) -> Distribution {
        Distribution {
            id: id.into(),
            name: id.into(),
            id_like: like.iter().map(|v| (*v).into()).collect(),
        }
    }

    #[test]
    fn os_release_quotes_do_not_execute_or_expand() {
        let release = parse_os_release(
            r#"ID=fedora
ID_LIKE='rhel centos'
PRETTY_NAME="$(touch /tmp/never) `id` \"quoted\""
"#,
        )
        .unwrap();
        assert_eq!(
            release["PRETTY_NAME"],
            "$(touch /tmp/never) `id` \"quoted\""
        );
    }
    #[test]
    fn os_release_rejects_unterminated_quotes() {
        assert!(parse_os_release("ID=\"fedora").is_err());
    }
    #[test]
    fn os_release_rejects_control_characters() {
        assert!(parse_os_release("NAME='hello\u{1b}'").is_err());
    }
    #[test]
    fn derivative_uses_native_family_not_first_installed_binary() {
        assert_eq!(
            select_system(distro("pop", &["ubuntu", "debian"]), false, false, |_| true).backend,
            Some(Backend::Apt)
        );
    }
    #[test]
    fn atomic_precedes_dnf() {
        assert_eq!(
            select_system(distro("fedora", &[]), true, true, |_| true).backend,
            Some(Backend::RpmOstree)
        );
    }
    #[test]
    fn steam_os_never_gets_mutable_pacman() {
        assert_eq!(
            select_system(distro("steamos", &["arch"]), false, false, |_| true).backend,
            None
        );
    }
    #[test]
    fn immutable_derivative_never_gets_apt() {
        assert_eq!(
            select_system(distro("custom", &["debian"]), false, true, |_| true).backend,
            None
        );
    }
    #[test]
    fn arch_prefers_paru() {
        assert_eq!(
            select_system(distro("arch", &[]), false, false, |_| true).backend,
            Some(Backend::Paru)
        );
    }
    #[test]
    fn unknown_distro_does_not_guess_manager() {
        assert_eq!(
            select_system(distro("unknown", &[]), false, false, |_| true).backend,
            None
        );
    }
    #[test]
    fn fedora_prefers_dnf5() {
        assert_eq!(
            select_system(distro("fedora", &[]), false, false, |_| true).backend,
            Some(Backend::Dnf5)
        );
    }
    #[test]
    fn native_dnf_100_is_success() {
        assert!(
            validate_status(
                "dnf5",
                &Captured {
                    code: 100,
                    stdout: "bash.x86_64 5.3-1 updates".into(),
                    stderr: String::new()
                },
                &[0, 100]
            )
            .is_ok()
        );
    }
    #[test]
    fn native_checkupdates_2_is_empty_success() {
        assert!(
            validate_status(
                "checkupdates",
                &Captured {
                    code: 2,
                    stdout: String::new(),
                    stderr: String::new()
                },
                &[0, 2]
            )
            .is_ok()
        );
    }
    #[test]
    fn native_zero_exit_without_metadata_errors_is_success() {
        assert!(
            validate_status(
                "apt-get",
                &Captured {
                    code: 0,
                    stdout: String::new(),
                    stderr: String::new()
                },
                &[0]
            )
            .is_ok()
        );
    }
    #[test]
    fn persistent_cache_reuses_metadata_after_unlocking() {
        let base = PrivateDirectory::new().unwrap();
        let first = PersistentCache::at(&base.0, "dnf5").unwrap();
        fs::write(first.path.join("metadata"), b"retained").unwrap();
        drop(first);
        let second = PersistentCache::at(&base.0, "dnf5").unwrap();
        assert_eq!(fs::read(second.path.join("metadata")).unwrap(), b"retained");
    }

    #[test]
    fn persistent_cache_rejects_overlapping_checks() {
        let base = PrivateDirectory::new().unwrap();
        let _first = PersistentCache::at(&base.0, "apt").unwrap();
        assert!(PersistentCache::at(&base.0, "apt").is_err());
    }

    #[test]
    fn persistent_cache_rejects_symlinked_application_directory() {
        let base = PrivateDirectory::new().unwrap();
        let elsewhere = PrivateDirectory::new().unwrap();
        std::os::unix::fs::symlink(&elsewhere.0, base.0.join("cosmic-applet-system-updates"))
            .unwrap();
        assert!(PersistentCache::at(&base.0, "arch").is_err());
    }

    #[test]
    fn persistent_cache_rejects_nonprivate_backend_directory() {
        let base = PrivateDirectory::new().unwrap();
        let first = PersistentCache::at(&base.0, "apt").unwrap();
        let path = first.path.clone();
        drop(first);
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(PersistentCache::at(&base.0, "apt").is_err());
    }

    #[test]
    fn persistent_cache_rejects_symlinked_lock() {
        let base = PrivateDirectory::new().unwrap();
        let first = PersistentCache::at(&base.0, "apt").unwrap();
        let path = first.path.clone();
        drop(first);
        fs::remove_file(path.join(".lock")).unwrap();
        fs::write(base.0.join("other"), b"unchanged").unwrap();
        std::os::unix::fs::symlink(base.0.join("other"), path.join(".lock")).unwrap();
        assert!(PersistentCache::at(&base.0, "apt").is_err());
    }

    #[test]
    fn persistent_cache_rejects_hard_linked_lock() {
        let base = PrivateDirectory::new().unwrap();
        let first = PersistentCache::at(&base.0, "apt").unwrap();
        let path = first.path.clone();
        drop(first);
        fs::hard_link(path.join(".lock"), base.0.join("other")).unwrap();
        assert!(PersistentCache::at(&base.0, "apt").is_err());
    }
    #[test]
    fn native_failure_is_not_empty_success() {
        assert!(
            validate_status(
                "checkupdates",
                &Captured {
                    code: 1,
                    stdout: String::new(),
                    stderr: String::new()
                },
                &[0, 2]
            )
            .is_err()
        );
    }
    #[test]
    fn metadata_failure_is_not_success_even_with_zero_exit() {
        assert!(validate_status("apt-get", &Captured { code: 0, stdout: String::new(), stderr: "W: Some index files failed to download. They have been ignored, or old ones used instead.".into() }, &[0]).is_err());
    }
    #[test]
    fn dnf_key_failure_explains_manual_user_cache_approval() {
        let error = dnf_check_error(
            Backend::Dnf5,
            "Failed to import OpenPGP key: confirmation unavailable".into(),
        );
        assert!(
            error.contains("dnf5 --refresh check-update")
                && error.contains("normal user")
                && error.contains("never approve new keys")
        );
    }
    #[test]
    fn dnf_parser_ignores_headers_and_repository_messages() {
        assert_eq!(parse_dnf("Updating repositories:\nLast metadata expiration check: 0:02:30 ago\nName Arch Version Repository\nbash.x86_64 5.3-1.fc44 updates\n").len(), 1);
    }
    #[test]
    fn dnf5_json_reads_native_sections() {
        assert_eq!(parse_dnf5(r#"{"upgrades":[{"name":"bash","arch":"x86_64","evr":"5.3-1.fc44","repository":"updates"}]}"#).unwrap(), vec![package("bash.x86_64", None, "5.3-1.fc44", Some("updates"))]);
    }
    #[test]
    fn apt_parser_reads_available_and_installed_versions() {
        assert_eq!(
            parse_apt("Listing...\nbash/stable 5.3-1 amd64 [upgradable from: 5.2-1]\n").unwrap(),
            vec![package("bash", Some("5.2-1"), "5.3-1", Some("stable"))]
        );
    }
    #[test]
    fn arrows_parser_rejects_error_lines() {
        assert!(parse_arrows("error: Could not fetch updates", "repositories").is_err());
    }
    #[test]
    fn apk_parser_preserves_hyphenated_names() {
        assert_eq!(
            parse_apk("Installed: Available:\nmy-package-1.0-r0 < 1.1-r0\n").unwrap(),
            vec![package("my-package", Some("1.0-r0"), "1.1-r0", None)]
        );
    }
    #[test]
    fn xbps_parser_skips_dependencies_and_removals() {
        assert_eq!(parse_xbps("bash-5.3_1 update x86_64 https://repo 100 90\nnew-1.0_1 install x86_64 https://repo 100 90\n").unwrap().len(), 1);
    }
    #[test]
    fn zypper_parser_uses_headers_not_fixed_positions() {
        assert_eq!(parse_zypper("S | Repository | Name | Current Version | Available Version | Arch\nv | main | bash | 5.2-1 | 5.3-1 | x86_64\n").unwrap(), vec![package("bash", Some("5.2-1"), "5.3-1", Some("main"))]);
    }
    #[test]
    fn atomic_reports_real_deployment_data() {
        assert_eq!(parse_atomic(r#"{"deployments":[{"booted":true,"version":"44.1","checksum":"old","osname":"fedora"}],"cached-update":{"version":"44.2","checksum":"new","origin":"fedora:fedora/44"}}"#, 0).unwrap().packages, vec![package("fedora", Some("44.1"), "44.2", Some("fedora:fedora/44"))]);
    }
    #[test]
    fn atomic_never_fabricates_an_available_deployment() {
        assert!(parse_atomic(r#"{"deployments":[{"booted":true}]}"#, 0).is_err());
    }
    #[test]
    fn atomic_native_unchanged_status_ignores_stale_cached_update() {
        assert!(
            parse_atomic(
                r#"{"deployments":[{"booted":true}],"cached-update":{"checksum":"stale"}}"#,
                77
            )
            .unwrap()
            .packages
            .is_empty()
        );
    }
    #[test]
    fn helper_updates_run_as_user() {
        let system = select_system(distro("arch", &[]), false, false, |_| true);
        assert!(!update_plan(&system).unwrap().steps[0].sudo);
    }
    #[test]
    fn tumbleweed_plan_uses_distribution_upgrade() {
        let system = select_system(distro("opensuse-tumbleweed", &[]), false, false, |_| true);
        assert_eq!(update_plan(&system).unwrap().steps[1].args, ["dup"]);
    }
    #[test]
    fn plans_never_auto_confirm_or_run_partial_pacman_refresh() {
        for id in [
            "fedora",
            "ubuntu",
            "arch",
            "opensuse-leap",
            "alpine",
            "void",
            "solus",
        ] {
            let system = select_system(distro(id, &[]), false, false, |_| true);
            for step in update_plan(&system).unwrap().steps {
                assert!(
                    !step
                        .args
                        .iter()
                        .any(|arg| matches!(arg.as_str(), "-y" | "--yes" | "--noconfirm" | "-Sy"))
                );
            }
        }
    }
}
