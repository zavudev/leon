//! Leon, by Zavu: a keyboard-first desktop orchestrator for coding agents.
//!
//! `main` wires the pieces together and gets out of the way: it opens the
//! local store, starts the engine on a Tokio runtime and opens the window.
//! From then on the UI reads the store and the engine keeps the store fresh
//! (see `docs/ARCHITECTURE.md`).

// A release build for Windows is a window, not a console program: no terminal
// opens behind it. (A debug build keeps the console, for its log.)
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod address;
mod avatar;
mod brand;
mod cli;
mod connect;
mod diagnose;
mod elsewhere;
mod engine;
mod format;
mod fuzzy;
mod icons;
mod keys;
mod launch;
mod logging;
mod menus;
mod pair;
mod product;
mod remote;
mod schema;
mod settings;
mod share;
mod theme;
mod ui;
mod usage;

use gpui_kit::{
    px, size, App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};
use leon_core::Store;
use leon_remote::{ProcessRunner, SshOptions};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// How long a git or ssh command may run before it is stopped, so a machine
/// that does not answer cannot hold the engine up.
const COMMAND_TIME_LIMIT: Duration = Duration::from_secs(30);

fn main() {
    // Before anything else: the platform names the application from this.
    brand::set_process_name();
    logging::init();

    // `leon host ...` runs the sharing service without a window.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("host") {
        std::process::exit(leon_host::cli::run(
            arguments[1..].to_vec(),
            product::data_dir(),
        ));
    }

    let options = match cli::parse(arguments) {
        Ok(cli::Command::Run(options)) => options,
        Ok(cli::Command::Help) => {
            println!("{}", cli::usage());
            return;
        }
        Ok(cli::Command::Version) => {
            println!("{} {}", product::PRODUCT_NAME, product::VERSION);
            return;
        }
        Ok(cli::Command::Diagnose(request)) => {
            std::process::exit(diagnose::run(&request));
        }
        Ok(cli::Command::DiagnoseFolderDialog { timeout }) => {
            std::process::exit(diagnose::run_folder_dialog(timeout));
        }
        Ok(cli::Command::DiagnoseElsewhere) => {
            std::process::exit(diagnose::run_sessions_elsewhere());
        }
        Ok(cli::Command::DiagnoseConnect { destination }) => {
            std::process::exit(diagnose::run_connect(&destination));
        }
        Ok(cli::Command::DiagnoseResume { agent }) => {
            std::process::exit(diagnose::run_resume(agent));
        }
        Err(message) => {
            eprintln!("{message}\n\n{}", cli::usage());
            std::process::exit(2);
        }
    };

    let data_dir = options.data_dir.clone().unwrap_or_else(product::data_dir);
    if let Err(error) = std::fs::create_dir_all(&data_dir) {
        eprintln!(
            "{}: cannot create {}: {error}",
            product::PRODUCT_NAME,
            data_dir.display()
        );
        std::process::exit(1);
    }
    let store = match Store::open(data_dir.join("leon.db")) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("{}: cannot open the store: {error}", product::PRODUCT_NAME);
            std::process::exit(1);
        }
    };

    // Imports, git and ssh run here; the UI has its own executor.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("engine")
        .build()
        .expect("the Tokio runtime starts");

    // Shared SSH connections need a short directory that only this user can
    // reach, made before the first connection. Without one every command
    // simply opens its own connection.
    let runtime_dir = product::runtime_dir();
    let ssh = match product::ensure_private_dir(&runtime_dir) {
        Ok(()) => SshOptions::for_this_platform(runtime_dir),
        Err(error) => {
            tracing::warn!(%error, "no directory for shared ssh connections");
            SshOptions::without_multiplexing()
        }
    };
    // This installation's long-term keys (for machines reached through a
    // relay) and the one place that decides where a command starts: here, or
    // through a relay on the other computer.
    let identity = match leon_link::Identity::load_or_create(&data_dir) {
        Ok(identity) => Arc::new(identity),
        Err(error) => {
            eprintln!(
                "{}: cannot load the identity: {error}",
                product::PRODUCT_NAME
            );
            std::process::exit(1);
        }
    };
    let hub = leon_remote::RelayHub::new(
        identity.clone(),
        leon_host::cli::computer_name(),
        runtime.handle().clone(),
    );
    let runner = Arc::new(
        leon_remote::RoutingRunner::new(
            ProcessRunner::with_time_limit(COMMAND_TIME_LIMIT),
            hub.clone(),
        )
        .with_time_limit(COMMAND_TIME_LIMIT),
    );
    let engine = engine::Engine::new(
        store,
        runner.clone(),
        ssh,
        leon_history::default_roots(),
        runtime.handle().clone(),
    );
    engine.set_icon_fetcher(Arc::new(avatar::CurlFetcher));
    // Sessions running in another terminal: processes are listed through the
    // same runner as every other command.
    engine.set_process_scanner(runner, Some(std::process::id()));
    let remote_services = Arc::new(remote::Remote {
        hub: hub.clone(),
        share: share::ShareService::new(identity, runtime.handle().clone(), &data_dir),
        handle: runtime.handle().clone(),
    });
    let backend = Rc::new(remote::RoutingBackend::new(hub, runtime.handle().clone()));
    // What the store holds is shown at once; once the settings are read (they
    // may say not to) the engine brings it up to date.
    let started = engine.clone();

    // The user's themes, before anything asks for a theme.
    let themes_dir = data_dir.join(ui::THEMES_FOLDER);
    let summary = theme::user::reload(&themes_dir);
    for id in &summary.loaded {
        tracing::info!(id = %id, folder = %themes_dir.display(), "loaded the user theme");
    }
    for id in &summary.invalid {
        tracing::warn!(id = %id, "a user theme has errors; it is listed as invalid");
    }
    for problem in theme::registry::problems() {
        if problem.is_error() {
            tracing::warn!("{}", problem.line());
        } else {
            tracing::info!("{}", problem.line());
        }
    }
    let theme_name = match options.theme_name.map(cli::resolve_theme).transpose() {
        Ok(chosen) => chosen,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let overrides = settings::Overrides {
        appearance: options.theme,
        theme: theme_name,
    };
    let settings_file = data_dir.join(settings::FILE_NAME);
    gpui_kit::application()
        .with_assets(icons::Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            theme::install_fonts(cx);
            // The families of the system, for the fonts a theme file asks for.
            theme::user::set_installed_fonts(cx.text_system().all_font_names());
            theme::user::reload(&themes_dir);
            settings::init(Some(settings_file), overrides, cx);
            settings::start_engine(cx, &started);
            menus::install(menus::Availability::default(), cx);

            let bounds = Bounds::centered(None, size(px(1240.), px(800.)), cx);
            let window = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(820.), px(520.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(product::PRODUCT_NAME.into()),
                    ..Default::default()
                }),
                app_id: Some(product::APP_ID.to_owned()),
                icon: brand::window_icon(),
                ..Default::default()
            };
            if !brand::set_dock_icon() {
                tracing::warn!("the Dock icon could not be set");
            }
            gpui_kit::open_window(window, cx, |window, cx| {
                cx.new(|cx| {
                    let options = ui::Options {
                        backend: backend.clone(),
                        remote: Some(remote_services.clone()),
                        ..ui::Options::default()
                    };
                    ui::Shell::new(engine, options, window, cx)
                })
            })
            .expect("the main window opens");

            // Closing the window ends the application on every platform.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });

    // Whatever the engine was doing is already in the store; stopping its
    // tasks with the runtime loses nothing.
    runtime.shutdown_background();
}
