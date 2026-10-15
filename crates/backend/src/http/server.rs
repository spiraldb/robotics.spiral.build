//! Native HTTP runtime for the Spiral Robotics backend, started from the
//! command line.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crate::account::SpiralSessions;
use crate::account::sign_in::PublicAddress;
use crate::projects::context::StudioContext;
use clap::Parser;
use spiral_robotics_storage::{
    ProviderKeys, ServiceEndpoint, SessionStore, SpiralConnectionSettings, StudioStore,
};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Long-lived requests get a short deadline instead of being allowed to hold
/// process exit open.
const SERVER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);
/// Where a standalone Studio listens, and where browsers reach it: Spiral's
/// sign-in returns only to `http://localhost:3000/callback` on a developer's
/// machine.
const DEFAULT_PORT: u16 = 3000;
const DEFAULT_PUBLIC_URL: &str = "http://localhost:3000";

/// Curate and train on robot datasets in SpiralDB from a browser.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Curate and train on robot datasets in SpiralDB from a browser",
    long_about = "Serves the Spiral Robotics Studio web UI, stores Studio state in local SQLite and datasets in SpiralDB. Robot arms are driven by the robot connector, which links itself to Studio with its workspace's token."
)]
struct StudioServerArguments {
    /// Address the web UI is served on.
    #[arg(long, value_name = "ADDR:PORT")]
    bind: Option<SocketAddr>,

    /// The origin browsers reach Studio at. Spiral's sign-in returns them to
    /// its `/callback`, which must be registered with Spiral's sign-in.
    #[arg(
        long,
        env = "SPIRAL_ROBOTICS_PUBLIC_URL",
        default_value = DEFAULT_PUBLIC_URL,
        value_name = "URL"
    )]
    public_url: String,

    #[command(subcommand)]
    one_off: Option<OneOff>,
}

/// Work run once against a project instead of serving it.
#[derive(Debug, clap::Subcommand)]
enum OneOff {
    /// One-off (October 2026): number the episodes of every dataset of
    /// `--project`, or of `--dataset`, in place of their UUIDs, as the user
    /// whose token `SPIRAL_ACCESS_TOKEN` holds.
    NumberEpisodes {
        #[arg(long)]
        project: String,
        #[arg(long)]
        dataset: Option<String>,
    },
}

impl OneOff {
    fn run(self) -> ExitCode {
        let Self::NumberEpisodes { project, dataset } = self;
        let Ok(token) = std::env::var("SPIRAL_ACCESS_TOKEN") else {
            eprintln!("error: set SPIRAL_ACCESS_TOKEN to a token of the project's user");
            return ExitCode::FAILURE;
        };
        let settings = SpiralConnectionSettings::new(
            ServiceEndpoint::environment_spiral().into_string(),
            ServiceEndpoint::environment_spiral_spfs().into_string(),
        )
        .and_then(|settings| settings.with_project(&project));
        let settings = match settings {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        };
        let (publisher, access) = spiral_robotics_storage::SpiralAccessPublisher::new();
        publisher.publish(Some(token));
        let connection = spiral_robotics_storage::SpiralConnection::new(settings, access);
        let numbered = tokio::runtime::Runtime::new()
            .map_err(|error| error.to_string())
            .and_then(|runtime| {
                runtime.block_on(spiral_robotics_storage::SpiralSdkDatabase::number_episodes(
                    &connection,
                    dataset.as_deref(),
                ))
            });
        match numbered {
            Ok(_) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        }
    }
}

/// Everything a Studio server needs before it listens: its address, its
/// browser bundle, and its state.
struct ServerStartup {
    bind: SocketAddr,
    address: PublicAddress,
    studio: StudioStore,
    public: PathBuf,
}

/// Command-line Studio process launcher.
pub struct StudioServer;

impl StudioServer {
    pub fn run() -> ExitCode {
        let arguments = StudioServerArguments::parse();
        if let Some(one_off) = arguments.one_off {
            return one_off.run();
        }
        let Some(public) = ServerStartup::dioxus_public_directory()
            .filter(|path| path.join("index.html").is_file())
        else {
            eprintln!(
                "error: the Dioxus client bundle is missing; from the workspace root, run `dx serve --fullstack --debug-symbols false @client --package spiral-robotics-frontend @server --package spiral-robotics-backend`, or use `dx bundle` with the same options and launch the bundled server"
            );
            return ExitCode::FAILURE;
        };
        ServerStartup::install_logging();
        tracing::debug!("Dioxus public directory: {}", public.display());

        // `dx serve` publishes its chosen address through the standard fullstack
        // environment. An explicit CLI value still wins for standalone launches.
        let bind = arguments.bind.unwrap_or_else(|| {
            SocketAddr::new(
                dioxus_cli_config::server_ip()
                    .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
                dioxus_cli_config::server_port().unwrap_or(DEFAULT_PORT),
            )
        });
        let address = match PublicAddress::parse(&arguments.public_url) {
            Ok(address) => address,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        };
        let studio = match StudioStore::open_default() {
            Ok(studio) => studio,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        };

        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("error: could not start the async runtime: {error}");
                return ExitCode::FAILURE;
            }
        };

        let startup = ServerStartup {
            bind,
            address,
            studio,
            public,
        };
        match runtime.block_on(startup.serve_until_ctrl_c()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        }
    }
}

impl ServerStartup {
    fn install_logging() {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    EnvFilter::new("spiral_robotics_backend=info,tower_http=warn")
                }),
            )
            .init();
    }

    /// Serve until the first Ctrl+C, then stop running work.
    async fn serve_until_ctrl_c(self) -> Result<()> {
        let bound = self.bind().await?;
        println!(
            "Spiral Robotics Studio is running at http://{}",
            bound.address
        );
        let (shutdown, requested) = watch::channel(false);
        let signal_task = tokio::spawn(ShutdownSignal::listen(shutdown));
        let served = bound.serve(requested).await;
        signal_task.abort();
        served
    }

    /// Open Studio's state and sessions, and listen on its address.
    async fn bind(self) -> Result<BoundServer> {
        let Self {
            bind,
            address,
            studio,
            public,
        } = self;
        studio.initialize().await.map_err(std::io::Error::other)?;
        let keys = Arc::new(ProviderKeys::from_environment());
        tracing::info!(?keys, "provider keys from the environment");
        let endpoints = SpiralConnectionSettings::new(
            ServiceEndpoint::environment_spiral().into_string(),
            ServiceEndpoint::environment_spiral_spfs().into_string(),
        )
        .map_err(std::io::Error::other)?;
        let sessions = SpiralSessions::new(
            endpoints.endpoint().to_owned(),
            SessionStore::new(studio.clone()),
            address,
        )
        .map_err(std::io::Error::other)?;
        let factory = crate::projects::ProjectFactory::new(
            crate::projects::ProjectDataAccess::new(studio, keys.clone()),
        );
        let projects = crate::projects::ProjectRegistry::new(factory, endpoints);
        let state = StudioContext::new(projects, Arc::new(sessions), keys);
        let listener = tokio::net::TcpListener::bind(bind).await?;
        let address = listener.local_addr()?;

        let app = super::web_assets::StudioWebAssets::new(public)
            .router(
                super::routes::StudioApi::router(state.clone()),
                crate::account::sign_in::BrowserSignIn::routes().with_state(state.clone()),
            )
            .layer(tower_http::trace::TraceLayer::new_for_http());
        Ok(BoundServer {
            listener,
            address,
            app,
            state,
        })
    }

    /// DX sets the public path during development; a distributed server keeps
    /// its browser bundle in the adjacent public directory.
    fn dioxus_public_directory() -> Option<PathBuf> {
        std::env::var_os("DIOXUS_PUBLIC_PATH")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()?
                    .parent()
                    .map(|path| path.join("public"))
            })
    }
}

/// A Studio server listening on its address, and the state it serves.
struct BoundServer {
    listener: tokio::net::TcpListener,
    address: SocketAddr,
    app: axum::Router,
    state: StudioContext,
}

impl BoundServer {
    /// Serve until `shutdown` turns true, then stop the projects' running work.
    async fn serve(self, shutdown: watch::Receiver<bool>) -> Result<()> {
        let Self {
            listener,
            app,
            state,
            ..
        } = self;
        // Both the server and this task observe the same edge. The server begins
        // graceful draining immediately; this task enforces a deadline so a
        // long-lived request cannot hold process exit open.
        let server_result = {
            let server = axum::serve(listener, app)
                .with_graceful_shutdown(ShutdownSignal::requested(shutdown.clone()))
                .into_future();
            tokio::pin!(server);

            tokio::select! {
                result = &mut server => Some(result),
            () = ShutdownSignal::requested(shutdown) => {
                    match tokio::time::timeout(SERVER_DRAIN_TIMEOUT, &mut server).await {
                        Ok(result) => Some(result),
                        Err(_) => {
                            tracing::warn!("server drain timed out; closing remaining connections");
                            None
                        }
                    }
                }
            }
        };
        state.projects().shutdown().await;
        if let Some(result) = server_result {
            result?;
        }
        Ok(())
    }
}

/// Notify every shutdown waiter on the first Ctrl+C and make the second press
/// an unconditional escape hatch. Once Tokio installs its signal handler the
/// operating system's default second-press behavior no longer applies, so it
/// has to be restored explicitly.
struct ShutdownSignal;

impl ShutdownSignal {
    async fn listen(shutdown: tokio::sync::watch::Sender<bool>) {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!("could not listen for Ctrl+C: {error}");
        }
        eprintln!("shutdown requested; press Ctrl+C again to force exit");
        let _ = shutdown.send(true);

        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("forcing exit");
            std::process::exit(130);
        }
    }

    async fn requested(mut shutdown: tokio::sync::watch::Receiver<bool>) {
        if *shutdown.borrow() {
            return;
        }
        while shutdown.changed().await.is_ok() {
            if *shutdown.borrow() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_waiters_release_when_the_signal_is_published() {
        let (sender, receiver) = tokio::sync::watch::channel(false);
        let waiter = tokio::spawn(ShutdownSignal::requested(receiver));

        sender.send(true).expect("the waiter is listening");
        tokio::time::timeout(std::time::Duration::from_millis(100), waiter)
            .await
            .expect("the waiter must not hang")
            .expect("the waiter task must finish");
    }

    #[tokio::test]
    async fn an_already_published_shutdown_does_not_wait_for_another_change() {
        let (_sender, receiver) = tokio::sync::watch::channel(true);
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            ShutdownSignal::requested(receiver),
        )
        .await
        .expect("an existing shutdown request is immediately visible");
    }
}
