//! HashiCorp Vault credentials for connections (see `db::Auth`):
//! - vault_kv: the password is one field of a KV secret (v1 or v2), e.g. `secret/data/team/db`.
//! - vault_dynamic: the database secrets engine (`database/creds/<role>`) creates a temporary
//!   user name and password on every connect.
//!
//! The Vault token comes from, in order: this session's earlier sign-in, `VAULT_TOKEN`,
//! `~/.vault-token` (what `vault login` leaves), else an OpenID Connect sign-in in the browser
//! like `vault login -method=oidc`: same callback (http://localhost:8250/oidc/callback), so roles
//! set up for the Vault CLI work as they are. Tokens are kept in memory only.

use crate::db::{Auth, ConnConfig};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The password syntax from before the explicit modes: `vault:path#field`.
const PREFIX: &str = "vault:";
/// The Vault CLI's OIDC callback port; roles list http://localhost:8250/oidc/callback.
const CALLBACK_PORT: u16 = 8250;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(180);

/// Turn a `vault:path#field` password (the syntax before the explicit modes) into the
/// matching mode. Returns whether the connection changed.
pub fn upgrade_legacy(c: &mut ConnConfig) -> bool {
    if c.auth != Auth::Password || !c.password.starts_with(PREFIX) {
        return false;
    }
    let Ok((path, field)) = parse_ref(&c.password) else {
        return false;
    };
    c.auth = if field.is_none() && path.contains("/creds/") {
        Auth::VaultDynamic
    } else {
        Auth::VaultKv
    };
    c.vault_path = path;
    c.vault_field = field.unwrap_or_default();
    c.password.clear();
    true
}

/// `vault:path#field` -> (path, field).
fn parse_ref(r: &str) -> Result<(String, Option<String>), String> {
    let rest = r
        .strip_prefix(PREFIX)
        .ok_or("not a vault: reference")?
        .trim()
        .trim_start_matches('/');
    // spaces around # are no part of path or field; vault:#pw has no path
    let (path, field) = match rest.split_once('#') {
        Some((p, f)) => (p.trim(), Some(f.trim()).filter(|f| !f.is_empty())),
        None => (rest, None),
    };
    if path.trim_start_matches('/').is_empty() {
        return Err(
            "empty Vault path: use vault:<path>#<field> or vault:database/creds/<role>".into(),
        );
    }
    Ok((path.to_string(), field.map(String::from)))
}

/// Tokens of this session, per Vault address.
static TOKENS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The cache key of a Vault address: https://vault:8200/ and https://vault:8200 are one Vault.
fn key(addr: &str) -> String {
    addr.trim().trim_end_matches('/').to_string()
}

fn cached(addr: &str) -> Option<String> {
    TOKENS.lock().unwrap().as_ref()?.get(&key(addr)).cloned()
}

fn remember(addr: &str, token: Option<String>) {
    let mut t = TOKENS.lock().unwrap();
    let map = t.get_or_insert_with(HashMap::new);
    match token {
        Some(tok) => map.insert(key(addr), tok),
        None => map.remove(&key(addr)),
    };
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        // The OS trust store, so company CAs trusted by the system work (ureq's default is a
        // bundled list of public CAs).
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

/// GET/POST a Vault API path; returns (status, JSON body).
fn call(
    addr: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    query: &[(&str, &str)],
    body: Option<Value>,
) -> Result<(u16, Value), String> {
    let url = format!(
        "{}/v1/{}",
        addr.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let a = agent();
    let res = if method == "POST" {
        let mut r = a.post(&url);
        if let Some(t) = token {
            r = r.header("X-Vault-Token", t);
        }
        r.send_json(body.unwrap_or(Value::Null))
    } else {
        let mut r = a.get(&url);
        for (k, v) in query {
            r = r.query(*k, *v);
        }
        if let Some(t) = token {
            r = r.header("X-Vault-Token", t);
        }
        r.call()
    };
    let mut res = res.map_err(|e| format!("Vault at {addr}: {e}"))?;
    let status = res.status().as_u16();
    let json = res.body_mut().read_json::<Value>().unwrap_or(Value::Null);
    Ok((status, json))
}

fn vault_errors(v: &Value) -> String {
    let list = |k: &str| {
        v[k].as_array()
            .map(|e| {
                e.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|s| !s.is_empty())
    };
    // a KV v2 path without data/ answers 404 with no errors, only a warning saying so
    list("errors")
        .or_else(|| list("warnings"))
        .unwrap_or_else(|| "no details".into())
}

fn token_valid(addr: &str, token: &str) -> bool {
    matches!(
        call(
            addr,
            "GET",
            "auth/token/lookup-self",
            Some(token),
            &[],
            None
        ),
        Ok((200, _))
    )
}

/// A working token other than `refused`: session cache, VAULT_TOKEN, ~/.vault-token, else
/// OIDC sign-in.
fn token(c: &ConnConfig, addr: &str, open: &dyn Fn(&str), refused: &str) -> Result<String, String> {
    if let Some(t) = cached(addr).filter(|t| t != refused && token_valid(addr, t)) {
        return Ok(t);
    }
    let from_env = std::env::var("VAULT_TOKEN").ok();
    let from_file = dirs::home_dir()
        .and_then(|h| std::fs::read_to_string(h.join(".vault-token")).ok())
        .map(|s| s.trim().to_string());
    for t in [from_env, from_file]
        .into_iter()
        .flatten()
        .filter(|t| !t.is_empty() && t != refused)
    {
        if token_valid(addr, &t) {
            remember(addr, Some(t.clone()));
            return Ok(t);
        }
    }
    let mount = if c.vault_mount.trim().is_empty() {
        "oidc"
    } else {
        c.vault_mount.trim()
    };
    let t = oidc_login(addr, mount, c.vault_role.trim(), CALLBACK_PORT, open)?;
    remember(addr, Some(t.clone()));
    Ok(t)
}

/// Random hex, from the OS-seeded hasher keys.
fn nonce() -> String {
    crate::secrets::new_id()
}

/// Percent-decoding for the callback's query string.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            // two hex digits: from_str_radix would also take "+5"
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
                continue;
            }
            (b'+', _) => out.push(b' '),
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_params(target: &str) -> HashMap<String, String> {
    target
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

/// OpenID Connect sign-in: ask Vault for the provider's URL, open it in the browser, wait for
/// the provider to send the browser back to localhost, trade code and state for a Vault token.
fn oidc_login(
    addr: &str,
    mount: &str,
    role: &str,
    port: u16,
    open: &dyn Fn(&str),
) -> Result<String, String> {
    let mount = mount.trim().trim_matches('/'); // "/oidc/" is the mount oidc
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("cannot listen on localhost:{port} for the Vault sign-in ({e}); is another `vault login` running?"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://localhost:{port}/oidc/callback");
    let client_nonce = nonce();
    let mut body = serde_json::json!({ "redirect_uri": redirect, "client_nonce": client_nonce });
    if !role.is_empty() {
        body["role"] = role.into();
    }
    let (status, v) = call(
        addr,
        "POST",
        &format!("auth/{mount}/oidc/auth_url"),
        None,
        &[],
        Some(body),
    )?;
    let auth_url = v["data"]["auth_url"].as_str().unwrap_or("").to_string();
    if status != 200 || auth_url.is_empty() {
        return Err(format!(
            "Vault OIDC sign-in ({mount}, role {:?}) failed: {}. The role must allow the redirect URI {redirect}.",
            if role.is_empty() { "default" } else { role },
            if status == 200 {
                "no auth URL".to_string()
            } else {
                vault_errors(&v)
            }
        ));
    }
    open(&auth_url);

    // Wait for the browser to come back with ?code=…&state=…
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let params = loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                // browsers open idle sockets ahead of time: never wait on one for long
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                // read the headers too: closing with them unread resets the connection
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|n| n > 0)
                    && !header.trim().is_empty()
                {
                    header.clear();
                }
                let target = line.split_whitespace().nth(1).unwrap_or("").to_string();
                if !target.starts_with("/oidc/callback") {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                let p = query_params(&target);
                let ok = p.contains_key("code");
                let msg = if ok {
                    "Signed in to Vault. You can close this tab and go back to Rusty Beaver."
                } else {
                    "Vault sign-in failed. Go back to Rusty Beaver for details."
                };
                let page = format!(
                    "<!doctype html><meta charset=utf-8><title>Rusty Beaver</title><body style=\"font:16px system-ui;margin:3rem\"><p>{msg}</p>"
                );
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                );
                break p;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err("Vault sign-in timed out: nobody finished it in the browser within 3 minutes".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    if let Some(err) = params.get("error") {
        return Err(format!(
            "Vault sign-in failed: {err} {}",
            params
                .get("error_description")
                .map(String::as_str)
                .unwrap_or("")
        )
        .trim()
        .to_string());
    }
    let (code, state) = (
        params.get("code").cloned().unwrap_or_default(),
        params.get("state").cloned().unwrap_or_default(),
    );
    let (status, v) = call(
        addr,
        "GET",
        &format!("auth/{mount}/oidc/callback"),
        None,
        &[
            ("code", &code),
            ("state", &state),
            ("client_nonce", &client_nonce),
        ],
        None,
    )?;
    match v["auth"]["client_token"].as_str() {
        Some(t) if status == 200 => Ok(t.to_string()),
        _ => Err(format!(
            "Vault did not accept the sign-in: {}",
            vault_errors(&v)
        )),
    }
}

/// Open a URL in the default browser (`RUSTY_BEAVER_BROWSER` replaces the command).
pub fn open_browser(url: &str) {
    use std::process::Command;
    let r = if let Ok(cmd) = std::env::var("RUSTY_BEAVER_BROWSER") {
        Command::new(cmd).arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else if cfg!(windows) {
        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = r {
        eprintln!("could not open the browser for the Vault sign-in: {e}; open this URL: {url}");
    }
}

/// A secret's field as text: JSON-written secrets can hold numbers and booleans too.
fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The connection with its Vault credentials filled in: the password (vault_kv), or user name
/// and password (vault_dynamic). Password connections come back unchanged.
pub fn resolve(c: &ConnConfig, open: &dyn Fn(&str)) -> Result<ConnConfig, String> {
    if c.auth == Auth::Password {
        return Ok(c.clone());
    }
    let path = c.vault_path.trim().trim_start_matches('/').to_string();
    if path.is_empty() {
        return Err(match c.auth {
            Auth::VaultKv => "Vault secret: enter the secret path, e.g. secret/data/team/db".into(),
            _ => "Vault dynamic credentials: enter the credentials path, e.g. database/creds/readonly".into(),
        });
    }
    let addr = Some(c.vault_addr.trim().to_string())
        .filter(|a| !a.is_empty())
        .or_else(|| std::env::var("VAULT_ADDR").ok())
        .ok_or("no Vault address: enter it in the connection, or set VAULT_ADDR")?;
    let mut tok = token(c, &addr, open, "")?;
    let (mut status, mut v) = call(&addr, "GET", &path, Some(&tok), &[], None)?;
    if status == 403 {
        // The token expired or lacks rights: sign in again once, with another token.
        remember(&addr, None);
        tok = token(c, &addr, open, &tok)?;
        (status, v) = call(&addr, "GET", &path, Some(&tok), &[], None)?;
    }
    if status != 200 {
        return Err(format!("Vault {path}: HTTP {status}: {}", vault_errors(&v)));
    }
    let data = &v["data"];
    let mut out = c.clone();
    // plain credentials now: resolving again (db::connect does) asks Vault no second time
    out.auth = Auth::Password;
    match c.auth {
        Auth::VaultKv => {
            let f = match c.vault_field.trim() {
                "" => "password",
                f => f,
            };
            // KV v2 nests the secret in data.data, KV v1 has it in data.
            let value = scalar(&data["data"][f])
                .or_else(|| scalar(&data[f]))
                .ok_or_else(|| format!("Vault secret {path} has no field {f:?}"))?;
            out.password = value;
        }
        _ => match (scalar(&data["username"]), scalar(&data["password"])) {
            (Some(u), Some(p)) => {
                out.user = u;
                out.password = p;
            }
            _ => {
                return Err(format!(
                    "Vault {path} returned no username/password: is it a database credentials path (database/creds/<role>)?"
                ));
            }
        },
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn old_vault_passwords_become_explicit_modes() {
        let mut kv = ConnConfig {
            password: "vault:secret/data/db#pw".into(),
            ..Default::default()
        };
        assert!(upgrade_legacy(&mut kv));
        assert_eq!(
            (
                kv.auth,
                kv.vault_path.as_str(),
                kv.vault_field.as_str(),
                kv.password.as_str()
            ),
            (Auth::VaultKv, "secret/data/db", "pw", "")
        );
        let mut dynamic = ConnConfig {
            password: "vault:database/creds/ro".into(),
            ..Default::default()
        };
        assert!(upgrade_legacy(&mut dynamic));
        assert_eq!(
            (dynamic.auth, dynamic.vault_path.as_str()),
            (Auth::VaultDynamic, "database/creds/ro")
        );
        let mut plain = ConnConfig {
            password: "vault-ish but plain".into(),
            ..Default::default()
        };
        assert!(!upgrade_legacy(&mut plain));
    }

    #[test]
    fn references_and_query_strings() {
        // one Vault, one cached token, with or without the trailing slash
        remember("http://cache-key.test:8200/", Some("t1".into()));
        assert_eq!(cached("http://cache-key.test:8200").as_deref(), Some("t1"));
        assert_eq!(decode("a%+5b%2Fc"), "a% 5b/c"); // %+5 is no escape (+ is a space)
        assert!(parse_ref("vault:#pw").is_err() && parse_ref("vault:/#pw").is_err()); // no path
        assert_eq!(
            parse_ref("vault:secret/data/db # pw").unwrap(),
            ("secret/data/db".to_string(), Some("pw".to_string()))
        );
        assert_eq!(
            parse_ref("vault:kv/data/app#password").unwrap(),
            ("kv/data/app".to_string(), Some("password".to_string()))
        );
        assert_eq!(
            parse_ref("vault:/database/creds/ro").unwrap(),
            ("database/creds/ro".to_string(), None)
        );
        assert!(parse_ref("vault:").is_err());
        let p = query_params("/oidc/callback?code=a%2Fb%3D&state=x+y&bad=%zz");
        assert_eq!(
            (p["code"].as_str(), p["state"].as_str(), p["bad"].as_str()),
            ("a/b=", "x y", "%zz")
        );
    }

    /// A tiny fake Vault + identity provider: answers the OIDC endpoints, a KV v2 secret and
    /// dynamic database credentials, checking the token.
    fn fake_vault() -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let me = base.clone();
        std::thread::spawn(move || {
            let mut redirect = String::new();
            for s in l.incoming() {
                let mut s = s.unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let (mut token, mut len) = (String::new(), 0usize);
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    let lower = h.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("x-vault-token:") {
                        token = v.trim().to_string();
                    }
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let target = line.split_whitespace().nth(1).unwrap().to_string();
                let (status, json, location) = if target.starts_with("/v1/auth/oidc/oidc/auth_url")
                {
                    let b: Value = serde_json::from_slice(&body).unwrap();
                    redirect = b["redirect_uri"].as_str().unwrap().to_string();
                    (
                        200,
                        serde_json::json!({ "data": { "auth_url": format!("{me}/idp") } })
                            .to_string(),
                        None,
                    )
                } else if target.starts_with("/idp") {
                    (
                        302,
                        String::new(),
                        Some(format!("{redirect}?code=c0de&state=st")),
                    )
                } else if target.starts_with("/v1/auth/oidc/oidc/callback") {
                    let p = query_params(&target);
                    let ok =
                        p["code"] == "c0de" && p["state"] == "st" && !p["client_nonce"].is_empty();
                    (
                        if ok { 200 } else { 400 },
                        serde_json::json!({ "auth": { "client_token": "s.fake" } }).to_string(),
                        None,
                    )
                } else if token != "s.fake" {
                    (403, r#"{"errors":["permission denied"]}"#.to_string(), None)
                } else if target.starts_with("/v1/auth/token/lookup-self") {
                    (200, "{}".to_string(), None)
                } else if target.starts_with("/v1/kv/team") {
                    (404, r#"{"errors":[],"warnings":["Invalid path for a versioned K/V secrets engine. See the API docs for the appropriate API endpoints to use. If using the Vault CLI, use 'vault kv get' for this operation."]}"#.to_string(), None)
                } else if target.starts_with("/v1/kv/data/pin") {
                    (
                        200,
                        r#"{"data":{"data":{"password":123456}}}"#.to_string(),
                        None,
                    )
                } else if target.starts_with("/v1/kv/data/app") {
                    (
                        200,
                        r#"{"data":{"data":{"password":"kv-secret"}}}"#.to_string(),
                        None,
                    )
                } else if target.starts_with("/v1/database/creds/ro") {
                    (200, r#"{"data":{"username":"v-ro-123","password":"dyn-secret"},"lease_duration":3600}"#.to_string(), None)
                } else {
                    (404, r#"{"errors":[]}"#.to_string(), None)
                };
                let loc = location
                    .map(|l| format!("Location: {l}\r\n"))
                    .unwrap_or_default();
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\n{loc}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
            }
        });
        base
    }

    #[test]
    fn oidc_sign_in_then_kv_and_dynamic_credentials() {
        let addr = fake_vault();
        // The "browser": follows the identity provider's redirect back to our callback listener.
        let browser = |url: &str| {
            let url = url.to_string();
            std::thread::spawn(move || {
                let _ = ureq::get(&url).call();
            });
        };
        let token = oidc_login(&addr, "oidc", "", 0, &browser).unwrap();
        assert_eq!(token, "s.fake");
        // a mount typed as /oidc/ is the mount oidc
        assert_eq!(
            oidc_login(&addr, "/oidc/", "", 0, &browser).unwrap(),
            "s.fake"
        );

        remember(&addr, Some(token));
        let c = ConnConfig {
            auth: Auth::VaultKv,
            vault_path: "kv/data/app".into(),
            vault_addr: addr.clone(),
            user: "app".into(),
            ..Default::default()
        };
        let r = resolve(&c, &browser).unwrap();
        assert_eq!((r.user.as_str(), r.password.as_str()), ("app", "kv-secret"));

        let c = ConnConfig {
            auth: Auth::VaultDynamic,
            vault_path: "database/creds/ro".into(),
            vault_addr: addr.clone(),
            ..Default::default()
        };
        let r = resolve(&c, &browser).unwrap();
        assert_eq!(
            (r.user.as_str(), r.password.as_str()),
            ("v-ro-123", "dyn-secret")
        );
        assert_eq!(resolve(&r, &browser).unwrap().password, "dyn-secret"); // no 2nd lease

        let c = ConnConfig {
            auth: Auth::VaultKv,
            vault_path: "kv/data/app".into(),
            vault_field: "nope".into(),
            vault_addr: addr,
            ..Default::default()
        };
        assert!(resolve(&c, &browser).unwrap_err().contains("no field"));
        // KV v2 without data/: Vault's warning says why, not "no details"
        let team = ConnConfig {
            vault_path: "kv/team".into(),
            ..c.clone()
        };
        assert!(resolve(&team, &browser).unwrap_err().contains("versioned"));
        // a secret written as JSON can hold a number
        let pin = ConnConfig {
            vault_path: "kv/data/pin".into(),
            vault_field: String::new(),
            ..c.clone()
        };
        assert_eq!(resolve(&pin, &browser).unwrap().password, "123456");

        let plain = ConnConfig {
            password: "plain".into(),
            ..Default::default()
        };
        assert_eq!(resolve(&plain, &browser).unwrap().password, "plain");
    }
}
