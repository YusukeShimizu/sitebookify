use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use prost::Message as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Re-run codegen when local protos or Buf module config changes.
    println!("cargo:rerun-if-changed=proto/sitebookify/v1/service.proto");
    println!("cargo:rerun-if-changed=proto/sitebookify/v1/manifest.proto");
    println!("cargo:rerun-if-changed=proto/sitebookify/v1/toc.proto");
    println!("cargo:rerun-if-changed=buf.yaml");
    println!("cargo:rerun-if-changed=buf.lock");

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let fds_path = out_dir.join("descriptor_set.binpb");

    // Use Buf to build a self-contained FileDescriptorSet (including imports),
    // then feed it to tonic-build. This avoids needing to vendor googleapis /
    // protovalidate protos into the repo or to manage protoc include paths.
    let status = Command::new("buf")
        .args(["build", "--as-file-descriptor-set", "-o"])
        .arg(&fds_path)
        .status()
        .map_err(|err| format!("failed to run `buf build`: {err}"))?;
    if !status.success() {
        return Err("`buf build` failed".into());
    }

    let fds_bytes = fs::read(&fds_path)?;
    let fds = tonic_build::FileDescriptorSet::decode(fds_bytes.as_slice())?;

    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_fds(fds)?;

    Ok(())
}
