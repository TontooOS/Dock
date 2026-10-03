//! Small process helpers the demo fallback needs.
//!
//! The dock is a pure Wayland client, so there is no X11 window list to ask;
//! the demo dot heuristic looks at the process table instead.

/// Whether a process with that exact name is running.
pub fn is_running(name: &str) -> bool {
    match std::process::Command::new("pgrep")
        .args(["-x", name])
        .output()
    {
        Ok(out) => out.status.success() && !out.stdout.is_empty(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_process_is_not_running() {
        assert!(!is_running("tontoo-definitely-not-a-process"));
    }

    #[test]
    fn the_shell_itself_is_found() {
        // `sh` exists on every TontooOS image and on the build host.
        assert!(is_running("sh") || is_running("bash"));
    }
}