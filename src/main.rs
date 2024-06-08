use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::chroot;
use tempfile::tempdir;

// Usage: your_docker.sh run <image> <command> <arg1> <arg2> ...
fn main() -> Result<()> {
    let tmp_dir = tempdir().with_context(|| "Tried to create temporary directory".to_string())?;

    let args: Vec<_> = std::env::args().collect();
    let command = &args[3];
    let target_chroot_path = tmp_dir
        .path()
        .join(command.strip_prefix('/').unwrap_or(command));
    fs::create_dir_all(target_chroot_path.parent().unwrap()).with_context(|| {
        "Tried to create directories to contain executable inside the chroot".to_string()
    })?;
    fs::copy(command, target_chroot_path)
        .with_context(|| "Tried to copy executable into chroot".to_string())?;

    fs::create_dir(tmp_dir.path().join("dev"))
        .with_context(|| "Tried to create dev directory inside chroot".to_string())?;
    fs::write(tmp_dir.path().join("dev/null"), b"")
        .with_context(|| "Tried to create empty /dev/null file inside chroot")?;

    chroot(tmp_dir.path())
        .with_context(|| format!("Tried to chroot into {}", tmp_dir.path().display()))?;

    let command_args = &args[4..];
    let output = std::process::Command::new(command)
        .args(command_args)
        .output()
        .with_context(|| {
            format!(
                "Tried to run '{}' with arguments {:?}",
                command, command_args
            )
        })?;

    // Cleanup
    // fs::remove_dir_all(tmp_dir.path())
    //     .with_context(|| "Tried to cleanup temporary directory".to_string())?;
    // drop(tmp_dir);

    let status_code = output.status.code().unwrap_or_default();
    let std_out = std::str::from_utf8(&output.stdout)?;
    print!("{}", std_out);
    let std_err = std::str::from_utf8(&output.stderr)?;
    eprint!("{}", std_err);
    std::process::exit(status_code);
}
