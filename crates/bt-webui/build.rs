use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by Cargo"),
    );
    let dist_dir = manifest_dir.join("dist");

    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("pnpm-lock.yaml").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("index.html").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("vite.config.ts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("tsconfig.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src").display()
    );
    println!("cargo:rerun-if-changed={}", dist_dir.display());

    run_pnpm(&manifest_dir, ["install", "--frozen-lockfile"]);
    run_pnpm(&manifest_dir, ["build"]);

    let assets = collect_assets(&dist_dir).unwrap_or_else(|error| {
        panic!(
            "failed to collect WebUI dist assets from {}: {error}",
            dist_dir.display()
        )
    });
    validate_assets(&assets);

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set by Cargo"));
    let generated = out_dir.join("webui_assets.rs");
    fs::write(generated, render_assets(&dist_dir, &assets))
        .expect("failed to write generated WebUI asset table");
}

fn validate_assets(assets: &[PathBuf]) {
    let has_index = assets.iter().any(|asset| asset == Path::new("index.html"));
    let has_script = assets
        .iter()
        .any(|asset| asset.extension().and_then(OsStr::to_str) == Some("js"));
    let has_style = assets
        .iter()
        .any(|asset| asset.extension().and_then(OsStr::to_str) == Some("css"));

    if !(has_index && has_script && has_style) {
        panic!(
            "WebUI dist is incomplete: index.html={has_index}, js={has_script}, css={has_style}"
        );
    }
}

fn run_pnpm<const N: usize>(manifest_dir: &Path, args: [&str; N]) {
    let status = Command::new("corepack")
        .arg("pnpm")
        .args(args)
        .current_dir(manifest_dir)
        .status()
        .unwrap_or_else(|error| panic!("failed to run corepack pnpm: {error}"));

    if !status.success() {
        panic!("corepack pnpm command failed with status {status}");
    }
}

fn collect_assets(dist_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut assets = Vec::new();
    collect_assets_inner(dist_dir, dist_dir, &mut assets)?;
    assets.sort();
    Ok(assets)
}

fn collect_assets_inner(root: &Path, dir: &Path, assets: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_assets_inner(root, &path, assets)?;
        } else if path.is_file() {
            let relative = path.strip_prefix(root).map_err(io::Error::other)?;
            assets.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn render_assets(dist_dir: &Path, assets: &[PathBuf]) -> String {
    let mut output = String::from("static EMBEDDED_ASSETS: &[EmbeddedAsset] = &[\n");
    for asset in assets {
        let disk_path = dist_dir.join(asset);
        let route = format!("/{}", asset.to_string_lossy().replace('\\', "/"));
        let content_type = content_type(asset);
        output.push_str("    EmbeddedAsset {\n");
        output.push_str(&format!("        path: \"{}\",\n", escape(&route)));
        output.push_str(&format!(
            "        content_type: \"{}\",\n",
            escape(content_type)
        ));
        output.push_str(&format!(
            "        bytes: include_bytes!(\"{}\"),\n",
            escape(&disk_path.display().to_string())
        ));
        output.push_str("    },\n");
    }
    output.push_str("];\n");
    output
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(OsStr::to_str) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}
