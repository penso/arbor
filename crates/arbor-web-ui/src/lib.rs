use std::path::PathBuf;

/// Returns the directory containing built web UI assets.
///
/// Resolution order:
/// 1. `ARBOR_WEB_UI_DIR` environment variable (runtime override)
/// 2. Relative to the running binary:
///    - macOS .app bundle: `../Resources/web-ui`
///    - Linux / generic:   `../share/arbor/web-ui`
/// 3. Development fallback: `CARGO_MANIFEST_DIR/app/dist`
pub fn dist_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ARBOR_WEB_UI_DIR") {
        return PathBuf::from(dir);
    }

    if let Some(dir) = resolve_relative_to_binary() {
        return dir;
    }

    // Development fallback: source tree
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("app")
        .join("dist")
}

pub fn dist_index_path() -> PathBuf {
    dist_dir().join("index.html")
}

pub fn dist_is_built() -> bool {
    dist_index_path().is_file()
}

/// Returns the app source directory (for development builds only).
pub fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("app")
}

fn resolve_relative_to_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bin_dir = exe.parent()?;

    // macOS .app bundle: Contents/MacOS/<binary> → Contents/Resources/web-ui
    let macos_path = bin_dir.join("../Resources/web-ui");
    if macos_path.join("index.html").is_file() {
        return Some(macos_path);
    }

    // Linux / generic: bin/<binary> → share/arbor/web-ui
    let share_path = bin_dir.join("../share/arbor/web-ui");
    if share_path.join("index.html").is_file() {
        return Some(share_path);
    }

    None
}
