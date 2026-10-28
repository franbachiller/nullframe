use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

include!("../tools/winres.rs");

fn main() {
    embed_windows_resource(Path::new("../assets/setup.rc"), "setup");
}
