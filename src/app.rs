use crate::i18n::{self, fill, t};
use crate::keys;
use crate::secrets;
use crate::ssh;
use crate::store::{Auth, Server, Store, parse_destination};
use crate::ui;
use anyhow::{Result, bail};
use inquire::validator::Validation;
use inquire::{Confirm, InquireError, Password, PasswordDisplayMode, Select, Text};
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
    /// Keep the password in the system keychain (or the config file).
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
        Err(t().name_empty.into())
    } else if name.chars().any(char::is_whitespace) {
        Err(t().name_spaces.into())
    } else if store.name_taken(name, except) {
        Err(fill(t().name_taken, &[name]))
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
    let mut prompt = Text::new(t().p_name)
        .with_placeholder(t().ph_name)
        .with_help_message(t().help_name)
        .with_validator(move |input: &str| {
            let name = input.trim();
            Ok(if name.is_empty() {
                Validation::Invalid(t().name_empty.into())
            } else if name.chars().any(char::is_whitespace) {
                Validation::Invalid(t().name_spaces.into())
            } else if taken.contains(&name.to_lowercase()) {
                Validation::Invalid(fill(t().name_taken, &[name]).into())
            } else {
                Validation::Valid
            })
        });
    if let Some(initial) = initial {
        prompt = prompt.with_initial_value(initial);
    }
    Ok(prompt.prompt()?.trim().to_string())
}

/// ssh would read a user or host starting with '-' as an option.
fn valid_destination(user: &str, host: &str) -> Result<(), String> {
    if host.is_empty() {
        Err(t().host_empty.into())
    } else if [user, host]
        .iter()
        .any(|part| part.starts_with('-') || part.chars().any(char::is_whitespace))
    {
        Err(t().bad_destination.into())
    } else {
        Ok(())
    }
}

fn prompt_host(initial: Option<&str>) -> Result<String> {
    let mut prompt = Text::new(t().p_host)
        .with_placeholder(t().ph_host)
        .with_help_message(t().help_host)
        .with_validator(|input: &str| {
            let (user, host, _) = parse_destination(input);
            Ok(
                match valid_destination(user.as_deref().unwrap_or(""), &host) {
                    Ok(()) => Validation::Valid,
                    Err(msg) => Validation::Invalid(msg.into()),
                },
            )
        });
    if let Some(initial) = initial {
        prompt = prompt.with_initial_value(initial);
    }
    Ok(prompt.prompt()?.trim().to_string())
}

fn prompt_user(default: &str) -> Result<String> {
    let user = Text::new(t().p_user)
        .with_default(default)
        .with_validator(|input: &str| {
            let user = input.trim();
            Ok(
                if user.starts_with('-') || user.chars().any(char::is_whitespace) {
                    Validation::Invalid(t().bad_destination.into())
                } else {
                    Validation::Valid
                },
            )
        })
        .prompt()?;
    Ok(if user.trim().is_empty() {
        default.to_string()
    } else {
        user.trim().to_string()
    })
}

fn prompt_port(default: u16) -> Result<u16> {
    let suggested = default.to_string();
    let help = fill(t().help_port, &[&suggested]);
    // No `with_default`: that draws "(22)" beside the cursor and also accepts
    // an empty Enter as 22. A spare newline from the previous question was
    // therefore saved as port 22 before another port could be typed.
    let raw = Text::new(t().p_port)
        .with_placeholder(&suggested)
        .with_help_message(&help)
        .with_validator(|input: &str| {
            Ok(if parse_port(input).is_some() {
                Validation::Valid
            } else {
                Validation::Invalid(t().port_error.into())
            })
        })
        .prompt()?;
    match parse_port(&raw) {
        Some(port) => Ok(port),
        None => bail!(t().port_error),
    }
}

/// A port the user actually typed. Empty input is rejected so a leftover
/// Enter cannot silently keep the suggestion.
fn parse_port(input: &str) -> Option<u16> {
    match input.trim().parse::<u16>() {
        Ok(port) if port >= 1 => Some(port),
        _ => None,
    }
}

fn prompt_password(help: &str) -> Result<String> {
    Ok(Password::new(t().p_password)
        .without_confirmation()
        .with_display_mode(PasswordDisplayMode::Masked)
        .with_display_toggle_enabled()
        .with_help_message(help)
        .prompt()?)
}

fn prompt_method() -> Result<Method> {
    let s = t();
    let pick = Select::new(s.p_method, vec![s.m_install, s.m_existing, s.m_password])
        .with_help_message(s.method_help)
        .raw_prompt()?;
    Ok(match pick.index {
        0 => Method::Install,
        1 => Method::Existing(choose_key()?),
        _ => Method::Password,
    })
}

fn read_password_stdin() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// Yes/no prompt that understands the current language's answers.
fn confirm(message: &str, default: bool) -> Result<bool> {
    let s = t();
    Ok(Confirm::new(message)
        .with_default(default)
        .with_default_value_formatter(&|d| {
            if d {
                s.yes_no_default_yes
            } else {
                s.yes_no_default_no
            }
            .to_string()
        })
        .with_formatter(&|answer| if answer { s.yes } else { s.no }.to_string())
        .with_parser(&|input| {
            let input = input.trim().to_lowercase();
            if s.yes_words.contains(&input.as_str()) {
                Ok(true)
            } else if s.no_words.contains(&input.as_str()) {
                Ok(false)
            } else {
                Err(())
            }
        })
        .with_error_message(s.yes_no_error)
        .prompt()?)
}

fn use_key(server: &mut Server, key: &str) {
    server.auth = Auth::Key;
    server.key = (key != keys::DEFAULT).then(|| key.to_string());
    server.password = None;
}

/// Installs `key` with the password, or falls back to saving the password
/// if that fails and the user agrees.
fn install_or_fallback(server: &mut Server, password: &str, key: &str) -> Result<()> {
    ui::step(&fill(t().installing, &[&server.address()]));
    match ssh::install_key(server, password, key) {
        Ok(()) => {
            use_key(server, key);
            ui::ok(t().key_installed);
            Ok(())
        }
        Err(e) => {
            ui::error(&fill(t().install_failed, &[&format!("{e:#}")]));
            let fallback = interactive() && !password.is_empty() && confirm(t().fallback_q, false)?;
            if !fallback {
                bail!(t().not_saved);
            }
            server.auth = Auth::Password;
            server.key = None;
            server.password = Some(password.to_string());
            Ok(())
        }
    }
}

fn setup_login(server: &mut Server, method: Method, password: Option<String>) -> Result<()> {
    let once = t().pw_once;
    match method {
        Method::Install => {
            let password = match password {
                Some(p) => p,
                None => prompt_password(once)?,
            };
            install_or_fallback(server, &password, keys::DEFAULT)
        }
        Method::Existing(key) => {
            ui::step(&fill(t().checking_key, &[&key, &server.address()]));
            match ssh::verify_key(server, &key) {
                Ok(()) => {
                    use_key(server, &key);
                    ui::ok(t().key_works);
                    Ok(())
                }
                Err(e) => {
                    ui::warn(&fill(t().key_not_accepted, &[&format!("{e:#}")]));
                    let password = match password {
                        Some(p) => p,
                        None if interactive() && confirm(t().install_q, true)? => {
                            prompt_password(once)?
                        }
                        None => bail!(t().not_saved),
                    };
                    install_or_fallback(server, &password, &key)
                }
            }
        }
        Method::Password => {
            let password = match password {
                Some(p) => p,
                None => prompt_password(t().pw_saved)?,
            };
            ui::step(&fill(t().checking_login, &[&server.address()]));
            if let Err(e) = ssh::check_password(server, &password) {
                ui::warn(&fill(t().login_failed, &[&format!("{e:#}")]));
                let keep = interactive() && confirm(t().save_anyway, false)?;
                if !keep {
                    bail!(t().not_saved);
                }
            } else {
                ui::ok(t().login_works);
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
    if let Err(msg) = valid_destination(&user, &host) {
        bail!(msg);
    }

    let method = match (opts.key, opts.auth) {
        (Some(key), _) => {
            if !keys::exists(&key) {
                bail!(fill(t().no_key_named, &[&key]));
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
        keychain: false,
        last_used: None,
    };
    setup_login(&mut server, method, password)?;

    let name = server.name.clone();
    store.servers.push(server);
    store.save()?;
    ui::ok(&fill(
        t().saved,
        &[
            &ui::bold(&name),
            &ui::accent("tuxius"),
            &ui::accent(&format!("tuxius {name}")),
        ],
    ));
    Ok(())
}

fn pick(store: &Store, message: &str) -> Result<usize> {
    let items = server_items(store);
    let choice = Select::new(message, items)
        .with_formatter(&short_answer)
        .with_page_size(12)
        .with_help_message(t().help_pick)
        .prompt()?;
    match choice.action {
        Action::Connect(i) => Ok(i),
        _ => unreachable!(),
    }
}

fn resolve(store: &Store, name: Option<String>, message: &str) -> Result<usize> {
    if store.servers.is_empty() {
        bail!(t().no_servers_err);
    }
    match name {
        Some(name) => store.find(&name),
        None => pick(store, message),
    }
}

pub fn connect(store: &mut Store, name: Option<String>, remote: &[String]) -> Result<()> {
    let idx = resolve(store, name, t().connect_to)?;
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
    let idx = resolve(store, name, t().edit_which)?;
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

    if let Err(msg) = valid_destination(&user, &host) {
        bail!(msg);
    }

    let mut server = Server {
        name,
        host,
        user,
        port,
        ..current.clone()
    };
    let renamed = server.name != current.name;
    if renamed && current.keychain {
        // The keychain entry is filed under the old name; re-save it under the new one.
        server.password = current.saved_password()?;
    }
    let moved =
        server.host != current.host || server.user != current.user || server.port != current.port;
    ui::step(&fill(t().currently, &[&current.auth_label()]));
    let change_login = confirm(t().change_login, moved && current.auth == Auth::Key)?;
    if change_login {
        let method = prompt_method()?;
        setup_login(&mut server, method, None)?;
    }

    store.servers[idx] = server;
    store.save()?;
    if current.keychain && (renamed || !store.servers[idx].keychain) {
        let _ = secrets::delete(&current.name);
    }
    ui::ok(&fill(t().updated, &[&ui::bold(&store.servers[idx].name)]));
    Ok(())
}

pub fn remove(store: &mut Store, name: Option<String>, yes: bool, revoke: bool) -> Result<()> {
    let idx = resolve(store, name, t().remove_which)?;
    let server = store.servers[idx].clone();
    if !yes {
        let sure = confirm(&fill(t().remove_q, &[&server.name]), false)?;
        if !sure {
            return Ok(());
        }
    }
    let revoke = server.auth == Auth::Key
        && (revoke
            || (!yes && interactive() && {
                ui::step(t().revoke_help);
                confirm(t().revoke_q, false)?
            }));
    if revoke {
        ui::step(&fill(t().removing_key, &[&server.address()]));
        match ssh::revoke_key(&server) {
            Ok(()) => ui::ok(t().key_removed_server),
            Err(e) => ui::warn(&fill(t().revoke_failed, &[&format!("{e:#}")])),
        }
    }
    store.servers.remove(idx);
    store.save()?;
    if server.keychain {
        let _ = secrets::delete(&server.name);
    }
    ui::ok(&fill(t().removed, &[&ui::bold(&server.name)]));
    Ok(())
}

pub fn list(store: &Store) {
    if store.servers.is_empty() {
        eprintln!("  {}", fill(t().list_empty, &[&ui::accent("tuxius add")]));
        return;
    }
    let s = t();
    let name_w = width(
        store.servers.iter().map(|s| s.name.clone()),
        s.col_name.chars().count(),
    );
    let addr_w = width(
        store.servers.iter().map(|s| s.address()),
        s.col_address.chars().count(),
    );
    println!(
        "  {}",
        ui::dim(&format!(
            "{:<name_w$}  {:<addr_w$}  {}",
            s.col_name, s.col_address, s.col_auth
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
    let mut prompt = Text::new(t().p_key_name)
        .with_placeholder(t().ph_key_name)
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
        None => Text::new(t().p_key_file)
            .with_placeholder("~/.ssh/id_ed25519")
            .with_help_message(t().help_key_file)
            .with_validator(|input: &str| {
                Ok(if keys::expand_home(input.trim()).is_file() {
                    Validation::Valid
                } else {
                    Validation::Invalid(t().file_not_found.into())
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
    ui::ok(&fill(t().imported, &[&ui::bold(&name)]));
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
    ui::ok(&fill(t().generated, &[&ui::bold(&name)]));
    eprintln!("  {}", ui::dim(t().pubkey_hint));
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
                Some(0) => format!("   {}", t().unused),
                Some(1) => format!("   {}", t().one_server),
                Some(n) => format!("   {}", fill(t().n_servers, &[&n.to_string()])),
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
    items.push(Item::new(t().import_item, Action::ImportKey));
    items.push(Item::new(t().generate_item, Action::GenerateKey));
    let choice = Select::new(t().which_key, items)
        .with_formatter(&short_answer)
        .with_help_message(t().help_cancel)
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
        bail!(t().no_keys_err);
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
            "  {}",
            fill(t().keys_empty, &[&ui::accent("tuxius key add")])
        );
        return Ok(());
    }
    let s = t();
    let name_w = width(
        list.iter().map(|k| k.name.clone()),
        s.col_name.chars().count(),
    );
    let kind_w = width(
        list.iter().map(|k| k.kind.clone()),
        s.col_type.chars().count(),
    );
    let fp_w = width(
        list.iter().map(|k| k.fingerprint.clone()),
        s.col_fingerprint.chars().count(),
    );
    println!(
        "  {}",
        ui::dim(&format!(
            "{:<name_w$}  {:<kind_w$}  {:<fp_w$}  {}",
            s.col_name, s.col_type, s.col_fingerprint, s.col_used_by
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
        let s = t();
        match Select::new(s.add_key, vec![s.import_opt, s.generate_opt])
            .raw_prompt()?
            .index
        {
            0 => import_key(name, None)?,
            _ => generate_key(name)?,
        };
    }
    Ok(())
}

pub fn key_show(store: &Store, name: Option<String>) -> Result<()> {
    let name = resolve_key(store, name, t().show_which)?;
    println!("{}", keys::public_key(&name)?);
    Ok(())
}

pub fn key_remove(store: &Store, name: Option<String>, yes: bool) -> Result<()> {
    let name = resolve_key(store, name, t().remove_key_which)?;
    let used = store.servers_using_key(&name);
    if !used.is_empty() {
        bail!(fill(t().key_in_use, &[&name, &used.join(", ")]));
    }
    if !yes {
        let sure = confirm(&fill(t().delete_key_q, &[&name]), false)?;
        if !sure {
            return Ok(());
        }
    }
    keys::remove(&name)?;
    ui::ok(&fill(t().deleted_key, &[&ui::bold(&name)]));
    Ok(())
}

pub fn keys_menu(store: &Store) -> Result<()> {
    loop {
        let mut items = key_items(Some(store))?;
        items.push(Item::new(t().import_item, Action::ImportKey));
        items.push(Item::new(t().generate_item, Action::GenerateKey));
        items.push(Item::new(t().back, Action::Back));
        let choice = Select::new(t().keys_title, items)
            .with_formatter(&short_answer)
            .with_page_size(14)
            .with_help_message(t().help_back)
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
    let s = t();
    let title = fill(s.key_title, &[&name]);
    match Select::new(&title, vec![s.show_pub, s.delete_key, s.back])
        .raw_prompt()?
        .index
    {
        0 => key_show(store, Some(name)),
        1 => key_remove(store, Some(name), false),
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
    Language,
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
    crate::update::announce(true);
    loop {
        if store.servers.is_empty() {
            eprintln!("  {}\n", t().first_server);
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
        let s = t();
        items.push(Item::new(s.menu_add, Action::Add));
        items.push(Item::new(s.menu_edit, Action::Edit));
        items.push(Item::new(s.menu_remove, Action::Remove));
        items.push(Item::new(s.menu_keys, Action::Keys));
        items.push(Item::new(s.menu_lang, Action::Language));
        items.push(Item::new(s.menu_quit, Action::Quit));

        let choice = match Select::new(s.connect_to, items)
            .with_formatter(&short_answer)
            .with_page_size(16)
            .with_help_message(s.help_menu)
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
            Action::Language => language(store, None),
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

/// Sets and saves the interface language; prompts when `code` is `None`.
pub fn language(store: &mut Store, code: Option<String>) -> Result<()> {
    let lang = match code {
        Some(code) => match i18n::Lang::parse(&code) {
            Some(lang) => lang,
            None => bail!(
                "unknown language '{code}' (available: {})",
                i18n::ALL.map(|l| l.code()).join(", ")
            ),
        },
        None => {
            let names: Vec<String> = i18n::ALL
                .iter()
                .map(|l| format!("{}  ({})", l.native_name(), l.code()))
                .collect();
            let start = i18n::ALL
                .iter()
                .position(|l| *l == i18n::current())
                .unwrap_or(0);
            let pick = Select::new(t().p_lang, names)
                .with_starting_cursor(start)
                .raw_prompt()?;
            i18n::ALL[pick.index]
        }
    };
    i18n::set(lang);
    store.lang = Some(lang.code().to_string());
    store.save()?;
    ui::ok(&fill(t().lang_set, &[lang.native_name()]));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_port;

    #[test]
    fn port_must_be_typed() {
        assert_eq!(parse_port(""), None);
        assert_eq!(parse_port("   "), None);
        assert_eq!(parse_port("0"), None);
        assert_eq!(parse_port("65536"), None);
        assert_eq!(parse_port("22"), Some(22));
        assert_eq!(parse_port(" 2222 "), Some(2222));
        assert_eq!(parse_port("65535"), Some(65535));
    }
}
