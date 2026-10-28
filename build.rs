//! Incrusta el icono y la información de versión en el ejecutable de Windows.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

include!("tools/winres.rs");

fn main() {
    embed_windows_resource(Path::new("assets/nullframe.rc"), "nullframe");
}
