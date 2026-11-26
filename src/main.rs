mod app;
mod i18n;
mod keys;
mod private;
mod secrets;
mod ssh;
mod store;
mod sync;
mod ui;
mod update;
mod vault;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use store::{Auth, Store};

#[derive(Parser)]
#[command(
    name = "tuxius",
    version,
    about = "Save your SSH servers once, connect with one keystroke.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Connect straight to a saved server (name or unique prefix)
    server: Option<String>,

    /// Run a command on the server instead of opening a shell
    #[arg(last = true)]
    remote: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Save a new server
    #[command(alias = "new")]
    Add {
        /// Nickname for the server
        #[arg(long)]
        name: Option<String>,
        /// IP or domain (user@host:port also works)
        #[arg(long)]
        host: Option<String>,
        /// Login user [default: root]
        #[arg(long, short)]
        user: Option<String>,
        /// SSH port [default: 22]
        #[arg(long, short)]
        port: Option<u16>,
        /// How to log in next time
        #[arg(long, value_enum, conflicts_with = "key")]
        auth: Option<AuthArg>,
        /// Log in with this key from `tuxius key list`
        #[arg(long, short)]
        key: Option<String>,
        /// Read the password from the first line of stdin (for scripts)
        #[arg(long)]
        password_stdin: bool,
    },
    /// List saved servers
    #[command(alias = "ls")]
    List,
    /// Connect to a saved server
    #[command(alias = "c")]
    Connect {
        name: Option<String>,
        /// Run a command on the server instead of opening a shell
        #[arg(last = true)]
        remote: Vec<String>,
    },
    /// Edit a saved server
    Edit { name: Option<String> },
    /// Remove a saved server
    #[command(alias = "rm")]
    Remove {
        name: Option<String>,
        /// Don't ask for confirmation
        #[arg(long, short)]
        yes: bool,
        /// Also remove the server's key from its authorized_keys
        #[arg(long)]
        revoke: bool,
    },
    /// Manage SSH keys (import, generate, list, delete)
    #[command(alias = "keys")]
    Key {
        #[command(subcommand)]
        action: Option<KeyCommand>,
    },
    /// Set the interface language: en, tr, ru (asks when omitted)
    #[command(alias = "language")]
    Lang { code: Option<String> },
    /// Show where Tuxius keeps its files
    Path,
    /// Download and install the latest Tuxius
    Update,
    /// Keep an encrypted copy of your servers in a cloud folder
    Sync {
        #[command(subcommand)]
        action: Option<SyncCommand>,
    },
}

#[derive(Subcommand)]
enum SyncCommand {
    /// Choose iCloud, a folder, or a WebDAV address
    Setup,
}

#[derive(Subcommand)]
enum KeyCommand {
    /// List stored keys
    #[command(alias = "ls")]
    List,
    /// Import a private key file, or generate a new one with --generate
    Add {
        /// Name for the key
        #[arg(long)]
        name: Option<String>,
        /// Private key file to import, e.g. ~/.ssh/id_ed25519
        #[arg(long, short, conflicts_with = "generate")]
        file: Option<String>,
        /// Generate a new ed25519 key instead of importing one
        #[arg(long, short)]
        generate: bool,
    },
    /// Print a key's public half (to paste into a server or cloud panel)
    #[command(alias = "pub")]
    Show { name: Option<String> },
    /// Delete a stored key
    #[command(alias = "rm")]
    Remove {
        name: Option<String>,
        /// Don't ask for confirmation
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum AuthArg {
    /// Install a new Tuxius key using the password once (recommended)
    Key,
    /// Save the password in the config file
    Password,
}

fn run(cli: Cli) -> Result<()> {
    ui::install_theme();
    let mut store = Store::load()?;
    i18n::set(i18n::detect(store.lang.as_deref()));
    if store.needs_migration() {
        store.save()?;
    }
    let menu = cli.command.is_none() && cli.server.is_none();
    let updating = matches!(cli.command, Some(Command::Update));
    if !menu && !updating {
        update::announce(false);
    }
    match (cli.command, cli.server) {
        (None, None) => app::menu(&mut store),
        (None, Some(name)) => app::connect(&mut store, Some(name), &cli.remote),
        (Some(cmd), _) => match cmd {
            Command::Add {
                name,
                host,
                user,
                port,
                auth,
                key,
                password_stdin,
            } => app::add(
                &mut store,
                app::AddOptions {
                    name,
                    host,
                    user,
                    port,
                    auth: auth.map(|a| match a {
                        AuthArg::Key => Auth::Key,
                        AuthArg::Password => Auth::Password,
                    }),
                    key,
                    password_stdin,
                },
            ),
            Command::List => {
                app::list(&store);
                Ok(())
            }
            Command::Connect { name, remote } => app::connect(&mut store, name, &remote),
            Command::Edit { name } => app::edit(&mut store, name),
            Command::Remove { name, yes, revoke } => app::remove(&mut store, name, yes, revoke),
            Command::Key { action } => match action {
                None => app::keys_menu(&mut store),
                Some(KeyCommand::List) => app::key_list(&store),
                Some(KeyCommand::Add {
                    name,
                    file,
                    generate,
                }) => app::key_add(name, file, generate),
                Some(KeyCommand::Show { name }) => app::key_show(&store, name),
                Some(KeyCommand::Remove { name, yes }) => app::key_remove(&mut store, name, yes),
            },
            Command::Lang { code } => app::language(&mut store, code),
            Command::Path => {
                println!("servers  {}", store.path().display());
                println!("keys     {}", keys::dir()?.display());
                if let Some(place) = sync::place() {
                    println!("sync     {place}");
                }
                Ok(())
            }
            Command::Update => update::perform(),
            Command::Sync { action } => match action {
                Some(SyncCommand::Setup) => sync::setup(&mut store),
                None => sync::run(&mut store),
            },
        },
    }
}

fn main() {
    if let Some(secret) = std::env::var_os(ssh::ASKPASS_ENV) {
        std::process::exit(ssh::askpass(&secret.to_string_lossy()));
    }

    if let Err(err) = run(Cli::parse()) {
        if app::is_cancel(&err) {
            std::process::exit(130);
        }
        ui::error(&format!("{err:#}"));
        std::process::exit(1);
    }
}
