#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[path = "../gui/mod.rs"]
mod gui;

use gui::{
    renderer::Mode,
    startup::{self, Backend, Stage},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq)]
struct Launch {
    mode: Mode,
    case: Option<PathBuf>,
    headless_check: bool,
}

fn launch_options(executable: &Path, arguments: &[OsString]) -> Result<Launch, String> {
    let safe_alias = executable
        .file_stem()
        .is_some_and(|name| name.eq_ignore_ascii_case("geosonnar-safe"));
    let mut result = Launch {
        mode: if safe_alias {
            Mode::SoftwareDirect3D12
        } else {
            Mode::Direct3D12
        },
        case: None,
        headless_check: false,
    };
    let mut explicit_renderer = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--case" && result.case.is_none() {
            result.case = Some(PathBuf::from(
                arguments
                    .next()
                    .ok_or("--case requires a saved case.json path")?,
            ));
        } else if argument == "--software-renderer" || argument == "--opengl" {
            if explicit_renderer || (safe_alias && argument == "--opengl") {
                return Err("Choose one renderer. The compatibility launcher always uses software rendering.".into());
            }
            explicit_renderer = true;
            result.mode = if argument == "--opengl" {
                Mode::OpenGl
            } else {
                Mode::SoftwareDirect3D12
            };
        } else if argument == "--headless-startup-check" && !result.headless_check {
            result.headless_check = true;
        } else {
            return Err("Supported options: --case PATH, --software-renderer, --opengl, --headless-startup-check".into());
        }
    }
    if result.headless_check && result.case.is_some() {
        return Err("The headless startup check does not open a case".into());
    }
    Ok(result)
}

fn main() {
    let _log = startup::begin();
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let launch = match launch_options(&std::env::current_exe().unwrap_or_default(), &arguments) {
        Ok(launch) => launch,
        Err(error) => {
            eprintln!("{error}");
            startup::mark(Stage::Exited);
            std::process::exit(2);
        }
    };
    let backend = match launch.mode {
        Mode::Direct3D12 => Backend::Direct3D12,
        Mode::SoftwareDirect3D12 => Backend::SoftwareDirect3D12,
        Mode::OpenGl => Backend::OpenGl,
    };
    startup::mark_backend(backend);
    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 820.0])
            .with_min_inner_size([760.0, 560.0])
            .with_resizable(true)
            .with_icon(gui::widgets::icon()),
        ..Default::default()
    };
    gui::renderer::configure(&mut options, launch.mode);
    if launch.headless_check {
        println!(
            "{}",
            serde_json::json!({"version":env!("CARGO_PKG_VERSION"), "renderer":format!("{:?}",launch.mode), "window_opened":false,"graphics_initialized":false,"recording_opened":false,"software_hardware_fallback":false})
        );
        startup::mark(Stage::Exited);
        return;
    }
    if let Err(error) = gui::shutdown::install_process_guard() {
        rfd::MessageDialog::new()
            .set_title("GeoSonnar could not establish safe shutdown")
            .set_description(format!("{error}\n\nThe window was not opened and no processing was started. GeoSonnar needs Windows to keep its helper processes together so Cancel all and exit can stop them. Try launching the program directly outside a restricted launcher. The command-line program remains available."))
            .set_level(rfd::MessageLevel::Error)
            .show();
        startup::mark(Stage::Exited);
        return;
    }
    if let Err(error) = eframe::run_native(
        concat!("GeoSonnar ", env!("CARGO_PKG_VERSION")),
        options,
        Box::new(move |cc| {
            startup::mark(Stage::CreationCallback);
            Ok(Box::new(gui::Workbench::new(cc, launch.case)))
        }),
    ) {
        startup::mark(Stage::EventLoopError);
        rfd::MessageDialog::new()
            .set_title("GeoSonnar could not start")
            .set_description(format!("{error}\n\nTry geosonnar-safe.exe from the same folder for software rendering. A startup log is saved beside the program when writable, otherwise in the temporary folder. The companion command-line program remains available."))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
    startup::mark(Stage::Exited);
}

#[cfg(test)]
mod launch_tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn normal_and_compatibility_launchers_choose_distinct_non_opengl_paths() {
        assert_eq!(
            launch_options(Path::new("geosonnar-gui.exe"), &[])
                .unwrap()
                .mode,
            Mode::Direct3D12
        );
        assert_eq!(
            launch_options(Path::new("GEOSONNAR-SAFE.exe"), &[])
                .unwrap()
                .mode,
            Mode::SoftwareDirect3D12
        );
        assert!(launch_options(Path::new("geosonnar-safe.exe"), &args(&["--opengl"])).is_err());
    }
    #[test]
    fn renderer_selection_preserves_explicit_case_and_rejects_conflicts() {
        let launch = launch_options(
            Path::new("geosonnar-gui.exe"),
            &args(&["--software-renderer", "--case", "My case/case.json"]),
        )
        .unwrap();
        assert_eq!(launch.case, Some(PathBuf::from("My case/case.json")));
        assert_eq!(launch.mode, Mode::SoftwareDirect3D12);
        for values in [
            vec!["--opengl", "--software-renderer"],
            vec!["--case"],
            vec!["--unknown"],
            vec!["--headless-startup-check", "--case", "case.json"],
        ] {
            assert!(launch_options(Path::new("geosonnar-gui.exe"), &args(&values)).is_err());
        }
    }
}
