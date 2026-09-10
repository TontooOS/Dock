//! App launching outside the Dock process.
//!
//! Clicking a tile never runs app code inside the dock. The launch is handed
//! to a detached helper instead:
//!
//! 1. Preferred: the LaunchPad daemon (`/run/launchpad.sock`, `start_app`).
//!    The daemon supervises the app as its own temp process, which is safer
//!    than a dock child (a crashing app can never take the dock down).
//! 2. Fallback: `tapp` (FishRunner, always installed at `/usr/bin/tapp` on
//!    TontooOS) is spawned detached with the bundle path. The dock never
//!    waits on it.
//!
//! The whole attempt runs on a throwaway thread so the GTK main loop is
//! never blocked, even when the daemon socket hangs.

use std::path::{Path, PathBuf};

/// LaunchPad daemon socket (see `TontooLibs/LaunchPad/src/types.rs`).
const LAUNCHPAD_SOCKET: &str = "/run/launchpad.sock";

/// `tapp` install location on TontooOS (FishRunner also registers a
/// `binfmt_misc` handler there, so it always exists on the system).
const TAPP_SYSTEM: &str = "/usr/bin/tapp";

/// Launch `bundle_path` (`....app`) as its own process, never in-process.
/// Returns immediately; success/failure is only logged.
pub fn launch_app(bundle_path: &Path, display_name: &str) {
    launch_with_args(bundle_path, display_name, &[]);
}

/// Launch `bundle_path` with extra passthrough arguments (after `--`).
/// Prefers the LaunchPad daemon only when no args are given (its
/// `start_app` takes no arguments); `tapp` receives the args directly.
pub fn launch_with_args(bundle_path: &Path, display_name: &str, args: &[String]) {
    let path = bundle_path.to_path_buf();
    let name = display_name.to_string();
    let args = args.to_vec();
    std::thread::spawn(move || {
        if args.is_empty() && try_launchpad_daemon(&path, &name) {
            return;
        }
        spawn_tapp(&path, &name, &args);
    });
}

/// Reveal `bundle_path` in the file manager: prefer the installed Finder
/// (`tapp <Finder.app> -- <dir>`), fall back to `xdg-open <dir>`.
pub fn open_in_finder(bundle_path: &Path) {
    let dir = bundle_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    let dir_str = dir.to_string_lossy().into_owned();
    std::thread::spawn(move || {
        let finder = crate::CoreWindows::list_programs().into_iter().find(|app| {
            app.bundle_id.eq_ignore_ascii_case("com.tontoo.finder")
                || app.display_name.eq_ignore_ascii_case("Finder")
        });
        if let Some(app) = finder {
            let tapp = find_tapp();
            let mut command = std::process::Command::new(&tapp);
            command
                .arg(&app.bundle_path)
                .arg("--")
                .arg(&dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            match command.spawn() {
                Ok(child) => println!(
                    "[dock] revealed {} in Finder (pid {})",
                    dir_str,
                    child.id()
                ),
                Err(err) => eprintln!("[dock] reveal in Finder failed: {err}"),
            }
            return;
        }
        match std::process::Command::new("xdg-open")
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => println!("[dock] revealed {dir_str} via xdg-open"),
            Err(err) => eprintln!("[dock] reveal {dir_str} failed: {err}"),
        }
    });
}

/// Ask the LaunchPad daemon to start the app (`start_app` op).
/// Returns `true` when the daemon accepted the request.
/// Unix only (the daemon socket); always `false` elsewhere.
#[cfg(unix)]
fn try_launchpad_daemon(bundle_path: &Path, display_name: &str) -> bool {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut stream = match UnixStream::connect(LAUNCHPAD_SOCKET) {
        Ok(stream) => stream,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    // Mirrors `launchpad_lib::IpcRequest` with `action = "start_app"`.
    let request = serde_json::json!({
        "action": "start_app",
        "service": null,
        "options": { "head": null, "app_path": bundle_path.to_string_lossy() },
    });
    let mut text = request.to_string();
    text.push('\n');
    if stream.write_all(text.as_bytes()).is_err() {
        return false;
    }
    let mut reply = String::new();
    if stream.read_to_string(&mut reply).is_err() {
        return false;
    }
    let accepted = serde_json::from_str::<serde_json::Value>(&reply)
        .ok()
        .and_then(|value| {
            value
                .get("success")
                .and_then(|flag| flag.as_bool())
        })
        .unwrap_or(false);
    if accepted {
        println!("[dock] launched {display_name} via launchpad daemon");
    }
    accepted
}

#[cfg(not(unix))]
fn try_launchpad_daemon(_bundle_path: &Path, _display_name: &str) -> bool {
    false
}

/// Spawn `tapp <bundle> [-- args...]` detached: stdio nulled, own process
/// group on Linux, never waited on by the dock.
fn spawn_tapp(bundle_path: &Path, display_name: &str, args: &[String]) {
    use std::process::Stdio;

    let tapp = find_tapp();
    let mut command = std::process::Command::new(&tapp);
    command.arg(bundle_path);
    if !args.is_empty() {
        command.arg("--").args(args);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    match command.spawn() {
        Ok(child) => println!(
            "[dock] launched {display_name} via {} (pid {})",
            tapp.display(),
            child.id()
        ),
        Err(err) => eprintln!(
            "[dock] launch failed for {display_name} ({}): {err}",
            bundle_path.display()
        ),
    }
}

/// Preferred `tapp` binary: `/usr/bin/tapp` on TontooOS, plain `tapp`
/// from `PATH` anywhere else (dev machines).
fn find_tapp() -> PathBuf {
    if Path::new(TAPP_SYSTEM).is_file() {
        PathBuf::from(TAPP_SYSTEM)
    } else {
        PathBuf::from("tapp")
    }
}
