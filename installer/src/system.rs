//! Instalación por usuario: sin permisos de administrador.
//!
//! - Archivos en `%LOCALAPPDATA%\Programs\Nullframe`.
//! - Accesos directos en el menú Inicio (y opcionalmente en el escritorio).
//! - Entrada en "Aplicaciones instaladas" bajo `HKCU`.

use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use crate::payload::{self, Payload};

pub const APP_NAME: &str = "Nullframe";
pub const APP_EXE: &str = "nullframe.exe";
pub const UNINSTALL_EXE: &str = "uninstall.exe";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Nullframe";

pub fn default_install_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Programs")
        .join(APP_NAME)
}

/// Carpeta donde está instalado ahora, según el registro.
pub fn installed_dir() -> Option<PathBuf> {
    let output = hidden("reg")
        .args(["query", UNINSTALL_KEY, "/v", "InstallLocation"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|line| line.contains("InstallLocation"))?;
    let value = line.split("REG_SZ").nth(1)?.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

/// Progreso compartido entre el hilo de trabajo y la interfaz.
#[derive(Clone, Default)]
pub struct Progress {
    pub fraction: f32,
    pub detail: String,
    pub result: Option<Result<(), String>>,
}

pub type SharedProgress = Arc<Mutex<Progress>>;

fn report(progress: &SharedProgress, fraction: f32, detail: &str) {
    if let Ok(mut state) = progress.lock() {
        state.fraction = fraction.clamp(0.0, 1.0);
        state.detail = detail.to_owned();
    }
}

pub fn install(
    payload: &Payload,
    dir: &Path,
    desktop_shortcut: bool,
    progress: &SharedProgress,
) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("No se pudo crear la carpeta de destino: {error}"))?;

    // La extracción ocupa el 90 % de la barra; el resto, accesos y registro.
    let total = payload.total_size().max(1) as f32;
    let mut done = 0_u64;
    for entry in &payload.entries {
        let dest = dir.join(&entry.name);
        let partial = dir.join(format!("{}.partial", entry.name));
        report(progress, done as f32 / total * 0.9, &format!("copiando {}", entry.name));
        payload
            .extract(entry, &partial, |bytes| {
                done += bytes;
                report(progress, done as f32 / total * 0.9, &format!("copiando {}", entry.name));
            })
            .map_err(|error| format!("No se pudo copiar {}: {error}", entry.name))?;
        replace_file(&partial, &dest).map_err(|error| {
            let _ = std::fs::remove_file(&partial);
            if error.kind() == io::ErrorKind::PermissionDenied {
                "Nullframe está abierto. Ciérralo y vuelve a intentarlo.".to_owned()
            } else {
                format!("No se pudo instalar {}: {error}", entry.name)
            }
        })?;
    }

    report(progress, 0.92, "preparando desinstalador");
    let uninstaller = dir.join(UNINSTALL_EXE);
    payload::stub_bytes()
        .and_then(|bytes| std::fs::write(&uninstaller, bytes))
        .map_err(|error| format!("No se pudo crear el desinstalador: {error}"))?;

    report(progress, 0.95, "creando accesos directos");
    create_shortcuts(&dir.join(APP_EXE), desktop_shortcut)?;

    report(progress, 0.98, "registrando la aplicación");
    register(dir, &uninstaller, payload.total_size())?;

    report(progress, 1.0, "listo");
    Ok(())
}

pub fn uninstall(dir: &Path, progress: &SharedProgress) -> Result<(), String> {
    report(progress, 0.2, "quitando accesos directos");
    remove_shortcuts();

    report(progress, 0.4, "quitando registro");
    let _ = hidden("reg").args(["delete", UNINSTALL_KEY, "/f"]).status();

    report(progress, 0.6, "eliminando archivos");
    for name in [APP_EXE, "ffmpeg.exe"] {
        let path = dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| {
                if error.kind() == io::ErrorKind::PermissionDenied {
                    "Nullframe está abierto. Ciérralo y vuelve a intentarlo.".to_owned()
                } else {
                    format!("No se pudo eliminar {name}: {error}")
                }
            })?;
        }
    }

    report(progress, 1.0, "listo");
    Ok(())
}

/// El desinstalador no puede borrarse a sí mismo mientras corre: deja a
/// `cmd` esperando unos segundos para eliminar la carpeta al salir.
pub fn schedule_folder_removal(dir: &Path) {
    let mut command = hidden("cmd");
    // `cmd` no entiende el escapado `\"` que aplica `Command::arg`, así que
    // la línea va tal cual. El directorio de trabajo, fuera de la carpeta.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.raw_arg(format!(
            "/c ping 127.0.0.1 -n 3 > nul & rmdir /s /q \"{}\"",
            dir.display()
        ));
    }
    let _ = command.current_dir(std::env::temp_dir()).spawn();
}

pub fn launch_app(dir: &Path) {
    let _ = Command::new(dir.join(APP_EXE)).current_dir(dir).spawn();
}

fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        std::fs::remove_file(to)?;
    }
    std::fs::rename(from, to)
}

fn create_shortcuts(target: &Path, desktop: bool) -> Result<(), String> {
    let workdir = ps_quote(&target.parent().unwrap_or(target).display().to_string());
    let target = ps_quote(&target.display().to_string());
    let mut script = format!(
        "$shell = New-Object -ComObject WScript.Shell;\
         function Link($dir) {{\
           $link = $shell.CreateShortcut((Join-Path $dir '{APP_NAME}.lnk'));\
           $link.TargetPath = {target}; $link.WorkingDirectory = {workdir};\
           $link.IconLocation = {target} + ',0'; $link.Description = 'Music into motion';\
           $link.Save() }};\
         Link ([Environment]::GetFolderPath('Programs'));"
    );
    if desktop {
        script.push_str("Link ([Environment]::GetFolderPath('Desktop'));");
    }
    run_powershell(&script).map_err(|error| format!("No se pudieron crear los accesos directos: {error}"))
}

fn remove_shortcuts() {
    let script = format!(
        "foreach ($f in 'Programs','Desktop') {{\
           $p = Join-Path ([Environment]::GetFolderPath($f)) '{APP_NAME}.lnk';\
           if (Test-Path $p) {{ Remove-Item $p -Force }} }}"
    );
    let _ = run_powershell(&script);
}

fn register(dir: &Path, uninstaller: &Path, bytes: u64) -> Result<(), String> {
    let icon = dir.join(APP_EXE).display().to_string();
    let uninstall = format!("\"{}\" --uninstall", uninstaller.display());
    let size_kb = (bytes / 1024).to_string();
    let location = dir.display().to_string();
    let values: [(&str, &str, &str); 9] = [
        ("DisplayName", "REG_SZ", APP_NAME),
        ("DisplayVersion", "REG_SZ", VERSION),
        ("Publisher", "REG_SZ", APP_NAME),
        ("DisplayIcon", "REG_SZ", &icon),
        ("InstallLocation", "REG_SZ", &location),
        ("UninstallString", "REG_SZ", &uninstall),
        ("EstimatedSize", "REG_DWORD", &size_kb),
        ("NoModify", "REG_DWORD", "1"),
        ("NoRepair", "REG_DWORD", "1"),
    ];
    for (name, kind, data) in values {
        let status = hidden("reg")
            .args(["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", data, "/f"])
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!("No se pudo registrar la aplicación ({name})."));
        }
    }
    Ok(())
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn run_powershell(script: &str) -> Result<(), String> {
    let status = hidden("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("PowerShell terminó con código {:?}", status.code()))
    }
}

/// Proceso sin ventana de consola.
fn hidden(program: &str) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
