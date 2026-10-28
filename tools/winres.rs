// Incluido desde build.rs e installer/build.rs.

/// Compila un `.rc` con `windres` (toolchain GNU) o `rc.exe` (MSVC) y lo
/// enlaza en los binarios. Si no hay compilador de recursos, la app sigue
/// compilando, solo que sin icono.
fn embed_windows_resource(rc: &Path, name: &str) {
    println!("cargo:rerun-if-changed={}", rc.display());
    println!("cargo:rerun-if-changed={}", rc.with_file_name("icon.ico").display());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let include = rc.parent().expect("carpeta del .rc");
    let (status, output) = if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        let output = out_dir.join(format!("{name}_resource.o"));
        let status = Command::new("windres")
            .arg("--include-dir")
            .arg(include)
            .arg("--output-format=coff")
            .arg("-i")
            .arg(rc)
            .arg("-o")
            .arg(&output)
            .status();
        (status, output)
    } else {
        let output = out_dir.join(format!("{name}_resource.res"));
        let status = Command::new("rc.exe")
            .arg("/nologo")
            .arg("/i")
            .arg(include)
            .arg("/fo")
            .arg(&output)
            .arg(rc)
            .status();
        (status, output)
    };

    match status {
        Ok(status) if status.success() => {
            println!("cargo:rustc-link-arg-bins={}", output.display());
        }
        _ => println!("cargo:warning=No se pudo compilar {}: el ejecutable no tendrá icono.", rc.display()),
    }
}
