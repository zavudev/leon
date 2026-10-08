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
mod agent_usage;
mod avatar;
mod brand;
mod cli;
mod connect;
mod diagnose;
mod elsewhere;
mod engine;
// The editor is the first caller of the file engine.
#[allow(dead_code)]
mod files;
mod format;
mod fuzzy;
mod history_report;
mod icons;
mod keys;
mod launch;
mod learn;
mod logging;
mod menus;
mod pair;
mod platform;
mod product;
mod remote;
mod schema;
mod search;
mod settings;
mod share;
mod theme;
mod ui;
mod updates;
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
        Ok(cli::Command::DiagnoseUpdate(request)) => {
            std::process::exit(diagnose::run_update(&request));
        }
        Ok(cli::Command::DiagnoseFolderDialog { timeout }) => {
            std::process::exit(diagnose::run_folder_dialog(timeout));
        }
        Ok(cli::Command::DiagnoseElsewhere) => {
            std::process::exit(diagnose::run_sessions_elsewhere());
        }
        Ok(cli::Command::DiagnoseUsage {
            network,
            no_network,
        }) => {
            let file = product::data_dir().join(settings::FILE_NAME);
            std::process::exit(diagnose::run_usage(&network, &no_network, &file));
        }
        Ok(cli::Command::DiagnoseHistory { agent, data_dir }) => {
            let dir = data_dir.unwrap_or_else(product::data_dir);
            std::process::exit(history_report::run(agent, &dir));
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

    // A build that was started by "Restart to update" waits until the one
    // before it has let go of the data (see `leon_update::launch`).
    leon_update::launch::wait_for_parent(Duration::from_secs(20));

    let data_dir = options.data_dir.clone().unwrap_or_else(product::data_dir);
    if let Err(error) = std::fs::create_dir_all(&data_dir) {
        eprintln!(
            "{}: cannot create {}: {error}",
            product::PRODUCT_NAME,
            data_dir.display()
        );
        std::process::exit(1);
    }

    // Updates, before anything is opened: what an earlier run left is looked
    // after, and an update that is ready is put in place, started and watched
    // (the automatic mode); this process then ends.
    let updater = updates::build_updater(&data_dir);
    let update_mode = updates::Mode::parse(
        settings::stored(&data_dir.join(settings::FILE_NAME))
            .value("updates_mode")
            .as_text()
            .unwrap_or_default(),
    );
    let relaunch = leon_update::launch::spawn_with(std::env::args_os().skip(1).collect());
    if let leon_update::launch::Outcome::Exit(code) = updater
        .startup()
        .run(update_mode == updates::Mode::Automatic, &relaunch)
    {
        std::process::exit(code);
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
        store.clone(),
        runner.clone(),
        ssh,
        leon_history::default_roots(),
        runtime.handle().clone(),
    );
    engine.set_icon_fetcher(Arc::new(avatar::CurlFetcher));
    // What the computers paired with this one share of their own Leon: pulled
    // from their Leon through the relay into this one's store.
    engine.set_relay_hub(hub.clone());
    // Usage limits: the files the agents keep are read through the runner; the
    // two network sources run only when their settings are on.
    engine.set_usage(
        Arc::new(leon_usage::network::SystemCredentials {
            home: dirs::home_dir().unwrap_or_default(),
        }),
        Arc::new(leon_usage::network::CurlHttp),
        Arc::new(|| chrono::Utc::now().timestamp()),
    );
    // Nothing is read (and no keychain prompt can appear) before the window
    // is up: the window starts the reading.
    engine.defer_usage();
    // Sessions running in another terminal: processes are listed through the
    // same runner as every other command.
    engine.set_process_scanner(runner, Some(std::process::id()));
    let remote_services = Arc::new(remote::Remote {
        hub: hub.clone(),
        share: share::ShareService::new(
            identity,
            runtime.handle().clone(),
            &data_dir,
            store.clone(),
        ),
        handle: runtime.handle().clone(),
    });
    let backend = Rc::new(remote::RoutingBackend::new(hub, runtime.handle().clone()));
    let update_service = updates::Service::with_updater(updater, runtime.handle().clone());
    let update_service_in = update_service.clone();
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
    let open_den = options.den;
    // Made-up lions for the Den, in a development build only: a way to
    // look at a full den without starting a dozen agents.
    let den_cast = if cfg!(debug_assertions) {
        std::env::var("LEON_DEN_CAST")
            .ok()
            .and_then(|count| count.parse::<usize>().ok())
            .map_or(0, |count| count.min(40))
    } else {
        0
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
            // The name and identity the system shows on a notification.
            cx.set_app_identity(product::APP_ID, product::PRODUCT_NAME);
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
                        updates: Some(update_service_in.clone()),
                        den_cast,
                        ..ui::Options::default()
                    };
                    let mut shell = ui::Shell::new(engine, options, window, cx);
                    if open_den {
                        shell.show_den(window, cx);
                    }
                    shell
                })
            })
            .expect("the main window opens");

            // A few seconds into a run that is the first after an update, the
            // update is confirmed: the version before is let go of.
            let confirming = update_service_in.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                confirming.confirm_started();
            })
            .detach();

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
    runtime.shutdown_timeout(Duration::from_secs(2));

    // "Restart to update" was confirmed: the new build is started, told to
    // wait for this process to let go (it has), and watched until it says it
    // is up; if it does not come up, the old one is put back and started.
    if let Some(version) = update_service.take_restart() {
        let startup = update_service.updater().startup();
        let code = match leon_update::launch::hand_over(&startup, &version, &relaunch) {
            leon_update::launch::Outcome::Exit(code) => code,
            leon_update::launch::Outcome::Continue => 0,
        };
        std::process::exit(code);
    }
}
