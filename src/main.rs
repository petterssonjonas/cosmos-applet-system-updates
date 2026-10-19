// SPDX-License-Identifier: MPL-2.0

mod app;
mod backend;
mod icons;
mod launcher;
mod model;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Cosmos System Updates: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [] => cosmic::applet::run::<app::AppModel>(()).map_err(|error| error.to_string()),
        [arg] if arg == "--help" || arg == "-h" => {
            println!(
                "Cosmos System Updates\n\nLaunch without arguments from the COSMIC panel.\n  --check                 Check for updates without installing anything\n  --run-updates BACKEND    Internal interactive terminal update runner\n  --help                  Show this help"
            );
            Ok(())
        }
        [arg] if arg == "--check" => {
            let system = backend::detect()?;
            println!(
                "{} · {}",
                system.distro.name,
                system.backend.map_or("unsupported", model::Backend::id)
            );
            if let Some(detail) = &system.detail {
                println!("{detail}");
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            let report = runtime.block_on(backend::check(&system))?;
            println!("{} updates available", report.packages.len());
            for package in report.packages {
                println!(
                    "{}  {} -> {}{}",
                    package.name,
                    package.installed.as_deref().unwrap_or("installed"),
                    package.available,
                    package
                        .source
                        .map(|source| format!("  [{source}]"))
                        .unwrap_or_default()
                );
            }
            if let Some(note) = report.note {
                println!("{note}");
            }
            Ok(())
        }
        [arg, value] if arg == "--run-updates" => {
            let backend = value
                .to_str()
                .and_then(model::Backend::parse)
                .ok_or("Unknown update backend. Launch updates from the applet.")?;
            launcher::run(backend)
        }
        _ => Err("Unrecognized arguments. Use --help.".into()),
    }
}
