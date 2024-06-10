use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::chroot;
use std::path::Path;
use tempfile::tempdir;

static DOCKER_HUB: &str = "registry.hub.docker.com";

// Usage: your_docker.sh run <image> <command> <arg1> <arg2> ...
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();

    let image = &args[2];
    let image_parts: Vec<&str> = image.split(':').collect();
    let image_name = image_parts.first().unwrap();
    let mut image_tag = "latest";
    if image_parts.len() > 1 {
        image_tag = image_parts.get(1).unwrap();
    }

    let auth_token = get_auth_token(image_name)?;
    let layers = fetch_image_manifest(image_name, image_tag, &auth_token)?;

    let tmp_dir = tempdir().with_context(|| "Tried to create temporary directory".to_string())?;

    fetch_image_layers(layers, image_name, &auth_token, tmp_dir.path())?;

    let command = &args[3];
    let target_chroot_path = tmp_dir
        .path()
        .join(command.strip_prefix('/').unwrap_or(command));
    fs::create_dir_all(target_chroot_path.parent().unwrap()).with_context(|| {
        "Tried to create directories to contain executable inside the chroot".to_string()
    })?;
    fs::copy(command, target_chroot_path)
        .with_context(|| "Tried to copy executable into chroot".to_string())?;

    // /dev/null might already exist depending on the layers we pull, fail silently
    let _ = fs::create_dir(tmp_dir.path().join("dev"));
    let _ = fs::write(tmp_dir.path().join("dev/null"), b"");

    chroot(tmp_dir.path())
        .with_context(|| format!("Tried to chroot into {}", tmp_dir.path().display()))?;

    unsafe {
        libc::unshare(libc::CLONE_NEWPID);
    }

    // Run the command
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

/// Retrieves an auth token from dockerhub
///
/// This implementation is limited to using dockerhub (hostname s not configurable) and only grabs
/// a token with the pull scope.
///
/// See: https://distribution.github.io/distribution/spec/auth/jwt/
fn get_auth_token(image_name: &str) -> Result<String, anyhow::Error> {
    let auth_response = reqwest::blocking::get(format!(
        "https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/{}:pull",
        image_name
    ))
    .context("Tried to request an auth token")?;
    let raw_data = auth_response.text().unwrap();
    let parsed_response: Value = serde_json::from_str(raw_data.as_str())
        .context("Tried to parse docker registry's auth response")?;
    Ok(String::from(parsed_response["token"].as_str().unwrap()))
}

/// Retrieves an image's manifest
///
/// See: https://distribution.github.io/distribution/spec/api/#pulling-an-image-manifest
fn fetch_image_manifest(
    image_name: &str,
    image_tag: &str,
    token: &str,
) -> Result<Vec<String>, anyhow::Error> {
    let client = reqwest::blocking::Client::new();

    let manifest_response = client
        .get(format!(
            "https://{}/v2/library/{}/manifests/{}",
            DOCKER_HUB, image_name, image_tag
        ))
        .bearer_auth(token)
        .header(
            "Accept",
            "application/vnd.docker.distribution.manifest.v2+json",
        )
        .send()
        .context("Tried fetching image manifest")?;
    let raw_data = manifest_response.text().unwrap();
    let parsed_response: Value =
        serde_json::from_str(&raw_data).context("Tried to parsed docker's manifest response")?;

    let mut layers: Vec<String> = Vec::new();
    let layers_arr = parsed_response["layers"]
        .as_array()
        .expect("No layers found in manifest response");
    layers.extend(
        layers_arr
            .iter()
            .map(|l| String::from(l["digest"].as_str().unwrap())),
    );

    Ok(layers)
}

/// Fetch the images and save them to disk
///
/// See: https://distribution.github.io/distribution/spec/api/#pulling-a-layer
fn fetch_image_layers(
    layers: Vec<String>,
    image_name: &str,
    token: &str,
    destination: &Path,
) -> Result<()> {
    let client = reqwest::blocking::Client::new();

    // TODO: Make this async
    for layer in layers {
        let blob_response = client
            .get(format!(
                "https://{}/v2/library/{}/blobs/{}",
                DOCKER_HUB, image_name, layer
            ))
            .bearer_auth(token)
            .send()
            .with_context(|| format!("Tried fetching layer {}", layer))?;
        let gzipped_tar_data = blob_response.bytes()?;

        let tar_data = GzDecoder::new(&gzipped_tar_data[..]);
        let mut archive = tar::Archive::new(tar_data);
        archive.set_preserve_permissions(true);
        archive.set_unpack_xattrs(true);
        archive
            .unpack(destination)
            .context(format!("Unable to unpack to {}", destination.display()))?;
    }

    Ok(())
}
