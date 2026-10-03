//! TPT Microgrid Sizer Pro — desktop shell (paid edition).
//!
//! A native host binary that opens the OS webview (WebView2 on Windows) and
//! serves the `trunk build --release --features pro` output of the appfront
//! DOM bundle from `dist/`. The free WASM edition of the same bundle ships
//! to the hub; the Pro difference is the `--features pro` build, which
//! compiles in the seasonal simulation, sizing optimization, and report
//! export.
//!
//! `dist/` is also embedded into the exe at compile time (see [`DIST`]), so
//! the file actually shipped to customers (the Gumroad zip) is the exe
//! alone. Resolve order for the bundle directory:
//! 1. `TPT_MICROGRID_DIST` environment variable (dev override),
//! 2. `./dist` next to the current working directory,
//! 3. `dist` next to the executable,
//! 4. the embedded copy, extracted once into the OS temp directory — this
//!    is the path a real end user's double-click actually takes.

use std::path::PathBuf;

use include_dir::{include_dir, Dir};
use tpt_appfront_webview::{Acl, AppBuilder, Capability, ParamKind, ParamSpec, WindowConfig};

const APP_ID: &str = "nz.co.tptsolutions.microgrid-sizer-pro";

/// The `trunk build --release` output, embedded at compile time. `build.ps1
/// -Pro` runs the trunk build before this crate, so `../dist` (relative to
/// this crate's `Cargo.toml`) always exists by the time this macro expands.
static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/../dist");

fn main() {
    let dist_dir = resolve_dist_dir();
    if !dist_dir.join("index.html").exists() {
        eprintln!(
            "tpt-microgrid-sizer-pro: no index.html under {} and embedded-asset extraction \
             failed — this build is missing its web bundle",
            dist_dir.display()
        );
        std::process::exit(1);
    }

    let result = AppBuilder::new(APP_ID)
        .with_window(WindowConfig {
            id: "main".to_string(),
            title: "TPT Microgrid Sizer Pro".to_string(),
            width: 1280,
            height: 900,
            dist_dir,
        })
        .with_single_instance(true)
        .with_acl(Acl {
            capabilities: vec![Capability {
                action: "open_external".to_string(),
                params: vec![ParamSpec {
                    name: "url".to_string(),
                    required: true,
                    kind: ParamKind::String,
                    default: None,
                }],
            }],
        })
        .run(|action, params| {
            if action == "open_external" {
                let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");
                // Belt-and-braces on top of the ACL's string-type check: only
                // ever hand the OS shell a tptsolutions.co.nz URL.
                if url.starts_with("https://tptsolutions.co.nz") {
                    let _ = open::that(url);
                }
            }
            Ok(())
        });

    if let Err(error) = result {
        eprintln!("tpt-microgrid-sizer-pro: {error:#}");
        std::process::exit(1);
    }
}

fn resolve_dist_dir() -> PathBuf {
    if let Ok(from_env) = std::env::var("TPT_MICROGRID_DIST") {
        return PathBuf::from(from_env);
    }
    let cwd_dist = PathBuf::from("dist");
    if cwd_dist.join("index.html").exists() {
        return cwd_dist;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let exe_dist = exe_dir.join("dist");
            if exe_dist.join("index.html").exists() {
                return exe_dist;
            }
        }
    }
    extract_embedded_dist()
}

/// Writes the embedded [`DIST`] contents to a stable temp directory (once —
/// subsequent launches reuse the extracted copy) and returns its path.
fn extract_embedded_dist() -> PathBuf {
    let out_dir = std::env::temp_dir().join(format!("{APP_ID}-dist"));
    if !out_dir.join("index.html").exists() {
        let _ = std::fs::create_dir_all(&out_dir);
        if let Err(error) = DIST.extract(&out_dir) {
            eprintln!("tpt-microgrid-sizer-pro: failed to extract embedded assets: {error:#}");
        }
    }
    out_dir
}
