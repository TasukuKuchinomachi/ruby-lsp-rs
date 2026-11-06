use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use steep_interface::steep_version::SteepVersion;

const STEEP_REPOSITORY_URL: &str = "https://github.com/soutaro/steep.git";

fn tag_commit(steep_version: SteepVersion) -> &'static str {
    match steep_version {
        SteepVersion::Version2_0_0 => "652568f78160da8cffefc6f62ac1dd6fca3d36a7",
        SteepVersion::Version2_1_0 => "3b75b367c9ad3d07f8f053c0c1f28e4bab560f5d",
    }
}

fn git(arguments: &[&str], directory: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let output = command
        .args(arguments)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`git {}` failed:\n{}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn steep_smoke_directory(repository_root: &Path, steep_version: SteepVersion) -> PathBuf {
    repository_root
        .join("target")
        .join("steep-smoke")
        .join(steep_version.name())
        .join("smoke")
}

pub fn steep_smoke_tests(
    repository_root: &Path,
    steep_version: SteepVersion,
) -> Result<PathBuf, String> {
    let steep_tag = format!("v{steep_version}");
    let steep_tag_commit = tag_commit(steep_version);
    let smoke_directory = steep_smoke_directory(repository_root, steep_version);
    let version_directory = smoke_directory
        .parent()
        .expect("the smoke directory is in the version directory")
        .to_path_buf();
    let fetched_commit_path = version_directory.join("fetched_commit");
    let fetched_commit = fs::read_to_string(&fetched_commit_path).unwrap_or_default();
    if smoke_directory.is_dir() && fetched_commit.trim() == steep_tag_commit {
        return Ok(smoke_directory);
    }
    if smoke_directory.exists() {
        fs::remove_dir_all(&smoke_directory)
            .map_err(|error| format!("cannot remove {}: {error}", smoke_directory.display()))?;
    }

    eprintln!(
        "Fetching the smoke tests of Steep {steep_tag} into {}",
        version_directory.display()
    );
    let clone_directory = version_directory.join("clone");
    if clone_directory.exists() {
        fs::remove_dir_all(&clone_directory)
            .map_err(|error| format!("cannot remove {}: {error}", clone_directory.display()))?;
    }
    fs::create_dir_all(&version_directory)
        .map_err(|error| format!("cannot create {}: {error}", version_directory.display()))?;

    let clone_path = clone_directory.to_string_lossy().to_string();
    git(
        &[
            "clone",
            "--quiet",
            "--depth=1",
            "--filter=blob:none",
            "--sparse",
            &format!("--branch={steep_tag}"),
            STEEP_REPOSITORY_URL,
            &clone_path,
        ],
        None,
    )?;
    git(&["sparse-checkout", "set", "smoke"], Some(&clone_directory))?;
    let commit = git(&["rev-parse", "HEAD"], Some(&clone_directory))?;
    if commit != steep_tag_commit {
        return Err(format!(
            "{steep_tag} of {STEEP_REPOSITORY_URL} points to {commit}, not {steep_tag_commit}"
        ));
    }

    fs::rename(clone_directory.join("smoke"), &smoke_directory)
        .map_err(|error| format!("cannot move the smoke tests: {error}"))?;
    fs::remove_dir_all(&clone_directory)
        .map_err(|error| format!("cannot remove {}: {error}", clone_directory.display()))?;
    fs::write(&fetched_commit_path, steep_tag_commit)
        .map_err(|error| format!("cannot write {}: {error}", fetched_commit_path.display()))?;
    Ok(smoke_directory)
}
