use crate::keys;
use crate::ssh;
use crate::store::{Auth, Server, Store, parse_destination};
use crate::ui;
use anyhow::{Result, bail};
use inquire::validator::Validation;
use inquire::{Confirm, CustomType, InquireError, Password, PasswordDisplayMode, Select, Text};
use std::fmt;
use std::io::{BufRead, IsTerminal};

#[derive(Default)]
pub struct AddOptions {
    pub name: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub auth: Option<Auth>,
    pub key: Option<String>,
    pub password_stdin: bool,
}

/// How a server should be logged into from now on.
enum Method {
    /// Use the password once to install the default Tuxius key.
    Install,
    /// Use a key from the Tuxius key store.
    Existing(String),
    /// Keep the password in the config file.
    Password,
}

pub fn is_cancel(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<InquireError>(),
        Some(InquireError::OperationCanceled | InquireError::OperationInterrupted)
    )
}

fn interactive() -> bool {
    std::io::stdin().is_terminal()
}

fn valid_name(store: &Store, name: &str, except: Option<usize>) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        Err("Name can't be empty".into())
    } else if name.chars().any(char::is_whitespace) {
        Err("Use a name without spaces, e.g. web-prod".into())
    } else if store.name_taken(name, except) {
        Err(format!("'{name}' is already saved"))
    } else {
        Ok(())
    }
}

fn prompt_name(store: &Store, initial: Option<&str>, except: Option<usize>) -> Result<String> {
    let taken: Vec<String> = store
        .servers
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != except)
        .map(|(_, s)| s.name.to_lowercase())
        .collect();
    let mut prompt = Text::new("Name")
        .with_placeholder("e.g. web-prod")
        .with_help_message("Short nickname you'll pick from the list")
        .with_validator(move |input: &str| {
            let name = input.trim();
            Ok(if name.is_empty() {
                Validation::Invalid("Name can't be empty".into())
            } else if name.chars().any(char::is_whitespace) {
                Validation::Invalid("Use a name without spaces, e.g. web-prod".into())
            } else if taken.contains(&name.to_lowercase()) {
                Validation::Invalid(format!("'{name}' is already saved").into())
            } else {
                Validation::Valid
            })
        });
    if let Some(initial) = initial {
        prompt = prompt.with_initial_value(initial);
    }
    Ok(prompt.prompt()?.trim().to_string())
}

fn prompt_host(initial: Option<&str>) -> Result<String> {
    let mut prompt = Text::new("Host")
        .with_placeholder("IP or domain, e.g. 203.0.113.10")
        .with_help_message("Tip: root@1.2.3.4:2222 also works")
        .with_validator(|input: &str| {
            Ok(if input.trim().is_empty() {
                Validation::Invalid("Host can't be empty".into())
            } else {
                Validation::Valid
            })
        });
    if let Some(initial) = initial {
        prompt = prompt.with_initial_value(initial);
    }
    Ok(prompt.prompt()?.trim().to_string())
}

fn prompt_user(default: &str) -> Result<String> {
    let user = Text::new("User").with_default(default).prompt()?;
    Ok(if user.trim().is_empty() {
        default.to_string()
    } else {
        user.trim().to_string()
    })
}

fn prompt_port(default: u16) -> Result<u16> {
    Ok(CustomType::<u16>::new("Port")
        .with_default(default)
        .with_error_message("Enter a port number between 1 and 65535")
        .prompt()?)
}

fn prompt_password(help: &str) -> Result<String> {
    Ok(Password::new("Password")
        .without_confirmation()
        .with_display_mode(PasswordDisplayMode::Masked)
        .with_display_toggle_enabled()
        .with_help_message(help)
        .prompt()?)
}

fn prompt_method() -> Result<Method> {
    const INSTALL: &str = "New Tuxius key (recommended)";
    const EXISTING: &str = "My own SSH key";
    const PASS: &str = "Saved password";
    let pick = Select::new("Log in with", vec![INSTALL, EXISTING, PASS])
        .with_help_message("new key: password is used once, never stored · saved password: kept in config (chmod 600)")
        .prompt()?;
    Ok(match pick {
        INSTALL => Method::Install,
        EXISTING => Method::Existing(choose_key()?),
        _ => Method::Password,
    })
}

fn read_password_stdin() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn use_key(server: &mut Server, key: &str) {
    server.auth = Auth::Key;
    server.key = (key != keys::DEFAULT).then(|| key.to_string());
    server.password = None;
}

/// Installs `key` with the password, or falls back to saving the password
/// if that fails and the user agrees.
fn install_or_fallback(server: &mut Server, password: &str, key: &str) -> Result<()> {
    ui::step(&format!("Installing SSH key on {}…", server.address()));
    match ssh::install_key(server, password, key) {
        Ok(()) => {
            use_key(server, key);
            ui::ok("Key installed — no password needed from now on");
            Ok(())
        }
        Err(e) => {
            ui::error(&format!("Could not install key: {e:#}"));
            let fallback = interactive()
                && !password.is_empty()
                && Confirm::new("Save it with the password instead?")
                    .with_default(false)
                    .prompt()?;
            if !fallback {
                bail!("server not saved");
            }
            server.auth = Auth::Password;
            server.key = None;
            server.password = Some(password.to_string());
            Ok(())
        }
    }
}

fn setup_login(server: &mut Server, method: Method, password: Option<String>) -> Result<()> {
    const ONCE: &str = "Used to log in once — esc to cancel";
    match method {
        Method::Install => {
            let password = match password {
                Some(p) => p,
                None => prompt_password(ONCE)?,
            };
            install_or_fallback(server, &password, keys::DEFAULT)
        }
        Method::Existing(key) => {
            ui::step(&format!("Checking key '{key}' on {}…", server.address()));
            match ssh::verify_key(server, &key) {
                Ok(()) => {
                    use_key(server, &key);
                    ui::ok("Key login works");
                    Ok(())
                }
                Err(e) => {
                    ui::warn(&format!("The server doesn't accept this key yet ({e:#})"));
                    let password = match password {
                        Some(p) => p,
                        None if interactive()
                            && Confirm::new("Install it on the server using the password?")
                                .with_default(true)
                                .prompt()? =>
                        {
                            prompt_password(ONCE)?
                        }
                        None => bail!("server not saved"),
                    };
                    install_or_fallback(server, &password, &key)
                }
            }
        }
        Method::Password => {
            let password = match password {
                Some(p) => p,
                None => prompt_password("Saved in your config file (chmod 600)")?,
            };
            ui::step(&format!("Checking login to {}…", server.address()));
            if let Err(e) = ssh::check_password(server, &password) {
                ui::warn(&format!("Login check failed: {e:#}"));
                let keep =
                    interactive() && Confirm::new("Save anyway?").with_default(false).prompt()?;
                if !keep {
                    bail!("server not saved");
                }
            } else {
                ui::ok("Login works");
            }
            server.auth = Auth::Password;
            server.key = None;
            server.password = Some(password);
            Ok(())
        }
    }
}

pub fn add(store: &mut Store, opts: AddOptions) -> Result<()> {
    let scripted = opts.password_stdin || !interactive();
    let name = match opts.name {
        Some(name) => {
            if let Err(msg) = valid_name(store, &name, None) {
                bail!(msg);
            }
            name.trim().to_string()
        }
        None => prompt_name(store, None, None)?,
    };

    let raw_host = match opts.host {
        Some(host) => host,
        None => prompt_host(None)?,
    };
    let (dest_user, host, dest_port) = parse_destination(&raw_host);

    let user = match (opts.user, dest_user) {
        (Some(u), _) | (None, Some(u)) => u,
        (None, None) if scripted => "root".into(),
        (None, None) => prompt_user("root")?,
    };
    let port = match (opts.port, dest_port) {
        (Some(p), _) | (None, Some(p)) => p,
        (None, None) if scripted => 22,
        (None, None) => prompt_port(22)?,
    };

    let method = match (opts.key, opts.auth) {
        (Some(key), _) => {
            if !keys::exists(&key) {
                bail!("no key named '{key}' (see `tuxius key list`)");
            }
            Method::Existing(key)
        }
        (None, Some(Auth::Password)) => Method::Password,
        (None, Some(Auth::Key)) => Method::Install,
        (None, None) if opts.password_stdin => Method::Install,
        (None, None) => prompt_method()?,
    };
    let password = if opts.password_stdin {
        Some(read_password_stdin()?)
    } else {
        None
    };

    let mut server = Server {
        name,
        host,
        user,
        port,
        auth: Auth::Key,
        key: None,
        password: None,
        last_used: None,
    };
    setup_login(&mut server, method, password)?;

    let name = server.name.clone();
    store.servers.push(server);
    store.save()?;
    ui::ok(&format!(
        "Saved {} — connect with {} or {}",
        ui::bold(&name),
        ui::accent("tuxius"),
        ui::accent(&format!("tuxius {name}"))
    ));
    Ok(())
}

fn pick(store: &Store, message: &str) -> Result<usize> {
    let items = server_items(store);
    let choice = Select::new(message, items)
        .with_formatter(&short_answer)
        .with_page_size(12)
        .with_help_message("↑↓ move · type to filter · enter select · esc back")
        .prompt()?;
    match choice.action {
        Action::Connect(i) => Ok(i),
        _ => unreachable!(),
    }
}

fn resolve(store: &Store, name: Option<String>, message: &str) -> Result<usize> {
    if store.servers.is_empty() {
        bail!("no servers saved yet — run `tuxius add`");
    }
    match name {
        Some(name) => store.find(&name),
        None => pick(store, message),
    }
}

pub fn connect(store: &mut Store, name: Option<String>, remote: &[String]) -> Result<()> {
    let idx = resolve(store, name, "Connect to")?;
    connect_index(store, idx, remote)
}

fn connect_index(store: &mut Store, idx: usize, remote: &[String]) -> Result<()> {
    store.servers[idx].touch();
    store.save()?;
    let server = &store.servers[idx];
    if remote.is_empty() {
        eprintln!(
            "  {} {} {}\n",
            ui::accent("→"),
            ui::bold(&server.name),
            ui::dim(&server.address())
        );
    }
    ssh::connect(server, remote)
}

pub fn edit(store: &mut Store, name: Option<String>) -> Result<()> {
    let idx = resolve(store, name, "Edit which server?")?;
    let current = store.servers[idx].clone();

    let name = prompt_name(store, Some(&current.name), Some(idx))?;
    let (dest_user, host, dest_port) = parse_destination(&prompt_host(Some(&current.host))?);
    let user = match dest_user {
        Some(u) => u,
        None => prompt_user(&current.user)?,
    };
    let port = match dest_port {
        Some(p) => p,
        None => prompt_port(current.port)?,
    };

    let mut server = Server {
        name,
        host,
        user,
        port,
        ..current.clone()
    };
    let moved =
        server.host != current.host || server.user != current.user || server.port != current.port;
    let change_login = Confirm::new("Change login method (password / key)?")
        .with_default(moved && current.auth == Auth::Key)
        .with_help_message(&format!("currently: {}", current.auth_label()))
        .prompt()?;
    if change_login {
        let method = prompt_method()?;
        setup_login(&mut server, method, None)?;
    }

    store.servers[idx] = server;
    store.save()?;
    ui::ok(&format!("Updated {}", ui::bold(&store.servers[idx].name)));
    Ok(())
}

pub fn remove(store: &mut Store, name: Option<String>, yes: bool, revoke: bool) -> Result<()> {
    let idx = resolve(store, name, "Remove which server?")?;
    let server = store.servers[idx].clone();
    if !yes {
        let sure = Confirm::new(&format!("Remove '{}'?", server.name))
            .with_default(false)
            .prompt()?;
        if !sure {
            return Ok(());
        }
    }
    let revoke = server.auth == Auth::Key
        && (revoke
            || (!yes
                && interactive()
                && Confirm::new("Also remove its key from the server (authorized_keys)?")
                    .with_default(false)
                    .with_help_message("You'll need the password to connect again")
                    .prompt()?));
    if revoke {
        ui::step(&format!("Removing key from {}…", server.address()));
        match ssh::revoke_key(&server) {
            Ok(()) => ui::ok("Key removed from the server"),
            Err(e) => ui::warn(&format!("Could not remove key from the server: {e:#}")),
        }
    }
    store.servers.remove(idx);
    store.save()?;
    ui::ok(&format!("Removed {}", ui::bold(&server.name)));
    Ok(())
}

pub fn list(store: &Store) {
    if store.servers.is_empty() {
        eprintln!(
            "  No servers saved yet. Add one with {}",
            ui::accent("tuxius add")
        );
        return;
    }
    let name_w = width(store.servers.iter().map(|s| s.name.clone()), 4);
    let addr_w = width(store.servers.iter().map(|s| s.address()), 7);
    println!(
        "  {}",
        ui::dim(&format!(
            "{:<name_w$}  {:<addr_w$}  AUTH",
            "NAME", "ADDRESS"
        ))
    );
    for i in store.ordered() {
        let s = &store.servers[i];
        println!(
            "  {}  {}  {}",
            ui::bold(&format!("{:<name_w$}", s.name)),
            format_args!("{:<addr_w$}", s.address()),
            ui::dim(&s.auth_label())
        );
    }
}

fn width(items: impl Iterator<Item = String>, min: usize) -> usize {
    items.map(|s| s.chars().count()).max().unwrap_or(0).max(min)
}

// ---------------------------------------------------------------- keys

fn prompt_key_name(initial: Option<&str>) -> Result<String> {
    let mut prompt = Text::new("Key name")
        .with_placeholder("e.g. work")
        .with_validator(|input: &str| {
            Ok(match keys::validate_name(input.trim()) {
                Ok(()) => Validation::Valid,
                Err(msg) => Validation::Invalid(msg.into()),
            })
        });
    if let Some(initial) = initial {
        prompt = prompt.with_initial_value(initial);
    }
    Ok(prompt.prompt()?.trim().to_string())
}

fn import_key(name: Option<String>, file: Option<String>) -> Result<String> {
    let file = match file {
        Some(f) => f,
        None => Text::new("Key file")
            .with_placeholder("~/.ssh/id_ed25519")
            .with_help_message("Path to the private key (not the .pub file)")
            .with_validator(|input: &str| {
                Ok(if keys::expand_home(input.trim()).is_file() {
                    Validation::Valid
                } else {
                    Validation::Invalid("File not found".into())
                })
            })
            .prompt()?,
    };
    let path = keys::expand_home(file.trim());
    let name = match name {
        Some(n) => n,
        None => {
            let suggested = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("key")
                .to_string();
            prompt_key_name(Some(&suggested))?
        }
    };
    keys::import(&name, &path)?;
    ui::ok(&format!("Imported key {}", ui::bold(&name)));
    Ok(name)
}

fn generate_key(name: Option<String>) -> Result<String> {
    let name = match name {
        Some(n) => {
            if let Err(msg) = keys::validate_name(&n) {
                bail!(msg);
            }
            n
        }
        None => prompt_key_name(None)?,
    };
    keys::generate(&name)?;
    ui::ok(&format!("Generated key {} (ed25519)", ui::bold(&name)));
    eprintln!(
        "  {}",
        ui::dim("Public key — add it to a server or a cloud panel:")
    );
    println!("{}", keys::public_key(&name)?);
    Ok(name)
}

/// `store` adds a "used by" column.
fn key_items(store: Option<&Store>) -> Result<Vec<Item>> {
    let list = keys::list()?;
    let name_w = width(list.iter().map(|k| k.name.clone()), 0);
    let kind_w = width(list.iter().map(|k| k.kind.clone()), 0);
    Ok(list
        .into_iter()
        .map(|k| {
            let used = match store.map(|s| s.servers_using_key(&k.name).len()) {
                None => String::new(),
                Some(0) => "   unused".to_string(),
                Some(1) => "   1 server".to_string(),
                Some(n) => format!("   {n} servers"),
            };
            Item {
                label: format!(
                    "{:<name_w$}   {:<kind_w$}   {}{used}",
                    k.name, k.kind, k.fingerprint
                ),
                short: k.name.clone(),
                action: Action::Key(k.name),
            }
        })
        .collect())
}

/// Lets the user pick a stored key, or import / generate one on the spot.
fn choose_key() -> Result<String> {
    let mut items = key_items(None)?;
    items.push(Item::new("+ Import a key file", Action::ImportKey));
    items.push(Item::new("+ Generate a new key", Action::GenerateKey));
    let choice = Select::new("Which key?", items)
        .with_formatter(&short_answer)
        .with_help_message("↑↓ move · enter select · esc cancel")
        .prompt()?;
    match choice.action {
        Action::Key(name) => Ok(name),
        Action::ImportKey => import_key(None, None),
        Action::GenerateKey => generate_key(None),
        _ => unreachable!(),
    }
}

fn resolve_key(store: &Store, name: Option<String>, message: &str) -> Result<String> {
    if let Some(name) = name {
        return Ok(name);
    }
    let items = key_items(Some(store))?;
    if items.is_empty() {
        bail!("no keys yet — add one with `tuxius key add`");
    }
    let choice = Select::new(message, items)
        .with_formatter(&short_answer)
        .prompt()?;
    match choice.action {
        Action::Key(name) => Ok(name),
        _ => unreachable!(),
    }
}

pub fn key_list(store: &Store) -> Result<()> {
    let list = keys::list()?;
    if list.is_empty() {
        eprintln!(
            "  No keys yet. Import or generate one with {}",
            ui::accent("tuxius key add")
        );
        return Ok(());
    }
    let name_w = width(list.iter().map(|k| k.name.clone()), 4);
    let kind_w = width(list.iter().map(|k| k.kind.clone()), 4);
    let fp_w = width(list.iter().map(|k| k.fingerprint.clone()), 11);
    println!(
        "  {}",
        ui::dim(&format!(
            "{:<name_w$}  {:<kind_w$}  {:<fp_w$}  USED BY",
            "NAME", "TYPE", "FINGERPRINT"
        ))
    );
    for k in list {
        let used = store.servers_using_key(&k.name).join(", ");
        println!(
            "  {}  {:<kind_w$}  {}  {}",
            ui::bold(&format!("{:<name_w$}", k.name)),
            k.kind,
            ui::dim(&format!("{:<fp_w$}", k.fingerprint)),
            if used.is_empty() {
                ui::dim("—")
            } else {
                used
            }
        );
    }
    Ok(())
}

pub fn key_add(name: Option<String>, file: Option<String>, generate: bool) -> Result<()> {
    if generate {
        generate_key(name)?;
    } else if file.is_some() {
        import_key(name, file)?;
    } else {
        const IMPORT: &str = "Import a key file";
        const NEW: &str = "Generate a new key";
        match Select::new("Add a key", vec![IMPORT, NEW]).prompt()? {
            IMPORT => import_key(name, None)?,
            _ => generate_key(name)?,
        };
    }
    Ok(())
}

pub fn key_show(store: &Store, name: Option<String>) -> Result<()> {
    let name = resolve_key(store, name, "Show which key?")?;
    println!("{}", keys::public_key(&name)?);
    Ok(())
}

pub fn key_remove(store: &Store, name: Option<String>, yes: bool) -> Result<()> {
    let name = resolve_key(store, name, "Remove which key?")?;
    let used = store.servers_using_key(&name);
    if !used.is_empty() {
        bail!(
            "key '{name}' is used by {} — remove those servers or switch them to another login first",
            used.join(", ")
        );
    }
    if !yes {
        let sure = Confirm::new(&format!("Delete key '{name}'? This can't be undone"))
            .with_default(false)
            .prompt()?;
        if !sure {
            return Ok(());
        }
    }
    keys::remove(&name)?;
    ui::ok(&format!("Deleted key {}", ui::bold(&name)));
    Ok(())
}

pub fn keys_menu(store: &Store) -> Result<()> {
    loop {
        let mut items = key_items(Some(store))?;
        items.push(Item::new("+ Import a key file", Action::ImportKey));
        items.push(Item::new("+ Generate a new key", Action::GenerateKey));
        items.push(Item::new("← Back", Action::Back));
        let choice = Select::new("SSH keys", items)
            .with_formatter(&short_answer)
            .with_page_size(14)
            .with_help_message("↑↓ move · enter select · esc back")
            .prompt()?;
        let result = match choice.action {
            Action::Key(name) => key_actions(store, name),
            Action::ImportKey => import_key(None, None).map(|_| ()),
            Action::GenerateKey => generate_key(None).map(|_| ()),
            _ => return Ok(()),
        };
        match result {
            Ok(()) => {}
            Err(e) if is_cancel(&e) => {}
            Err(e) => ui::error(&format!("{e:#}")),
        }
        eprintln!();
    }
}

fn key_actions(store: &Store, name: String) -> Result<()> {
    const SHOW: &str = "Show public key";
    const DELETE: &str = "Delete key";
    const BACK: &str = "← Back";
    match Select::new(&format!("Key '{name}'"), vec![SHOW, DELETE, BACK]).prompt()? {
        SHOW => key_show(store, Some(name)),
        DELETE => key_remove(store, Some(name), false),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------- menu

enum Action {
    Connect(usize),
    Add,
    Edit,
    Remove,
    Keys,
    Quit,
    Key(String),
    ImportKey,
    GenerateKey,
    Back,
}

struct Item {
    label: String,
    short: String,
    action: Action,
}

impl Item {
    fn new(label: &str, action: Action) -> Self {
        let short = label
            .split_once(' ')
            .map_or(label, |(_, rest)| rest)
            .to_string();
        Item {
            label: label.into(),
            short,
            action,
        }
    }
}

fn short_answer(option: inquire::list_option::ListOption<&Item>) -> String {
    option.value.short.clone()
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

fn server_items(store: &Store) -> Vec<Item> {
    let name_w = width(store.servers.iter().map(|s| s.name.clone()), 0);
    let addr_w = width(store.servers.iter().map(|s| s.address()), 0);
    store
        .ordered()
        .into_iter()
        .map(|i| {
            let s = &store.servers[i];
            Item {
                label: format!(
                    "{:<name_w$}   {:<addr_w$}   {}",
                    s.name,
                    s.address(),
                    s.auth_label()
                ),
                short: s.name.clone(),
                action: Action::Connect(i),
            }
        })
        .collect()
}

pub fn menu(store: &mut Store) -> Result<()> {
    ui::banner();
    loop {
        if store.servers.is_empty() {
            eprintln!("  No servers saved yet — let's add your first one.\n");
            match add(store, AddOptions::default()) {
                Ok(()) => {
                    eprintln!();
                    continue;
                }
                Err(e) if is_cancel(&e) => return Ok(()),
                Err(e) => return Err(e),
            }
        }

        let mut items = server_items(store);
        items.push(Item::new("+ Add a new server", Action::Add));
        items.push(Item::new("✎ Edit a server", Action::Edit));
        items.push(Item::new("✕ Remove a server", Action::Remove));
        items.push(Item::new("⚿ SSH keys", Action::Keys));
        items.push(Item::new("⏻ Quit", Action::Quit));

        let choice = match Select::new("Connect to", items)
            .with_formatter(&short_answer)
            .with_page_size(15)
            .with_help_message("↑↓ move · type to filter · enter select · esc quit")
            .prompt()
        {
            Ok(choice) => choice,
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };

        let result = match choice.action {
            Action::Connect(i) => return connect_index(store, i, &[]),
            Action::Add => add(store, AddOptions::default()),
            Action::Edit => edit(store, None),
            Action::Remove => remove(store, None, false, false),
            Action::Keys => keys_menu(store),
            _ => return Ok(()),
        };
        match result {
            Ok(()) => {}
            Err(e) if is_cancel(&e) => {}
            Err(e) => ui::error(&format!("{e:#}")),
        }
        eprintln!();
    }
}
