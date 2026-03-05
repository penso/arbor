use std::path::PathBuf;

pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn app_dir() -> PathBuf {
    crate_dir().join("app")
}

pub fn dist_dir() -> PathBuf {
    app_dir().join("dist")
}

pub fn dist_index_path() -> PathBuf {
    dist_dir().join("index.html")
}

pub fn dist_is_built() -> bool {
    dist_index_path().is_file()
}
