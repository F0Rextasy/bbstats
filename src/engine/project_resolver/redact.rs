use super::cmdline::split_cmdline_preserving_quotes;

pub(super) fn strip_shell_redirects(cmdline: &str) -> String {
    let tokens = split_cmdline_preserving_quotes(cmdline);
    let mut kept = Vec::with_capacity(tokens.len());
    for tok in tokens {
        if tok.starts_with(">>")
            || tok.starts_with("1>>")
            || tok.starts_with("2>>")
            || tok.starts_with('>')
            || tok.starts_with("1>")
            || tok.starts_with("2>")
            || tok == "2>&1"
        {
            break;
        }
        kept.push(tok);
    }
    kept.join(" ")
}

fn exe_basename_lower(tokens: &[String]) -> String {
    tokens
        .first()
        .map(|t| {
            let clean = t.trim_matches(['"', '\'']);
            let leaf = clean.rsplit(['/', '\\']).next().unwrap_or(clean);
            let low = leaf.to_ascii_lowercase();
            low.strip_suffix(".exe").unwrap_or(&low).to_string()
        })
        .unwrap_or_default()
}

fn is_password_dash_p_exe(exe: &str) -> bool {
    matches!(exe, "mysql" | "mariadb" | "sqlcmd" | "7z" | "unzip" | "openssl")
}

fn looks_like_port_spec(tok: &str) -> bool {
    let t = tok.trim_matches(['"', '\'']);
    if t.is_empty() {
        return false;
    }
    if t.len() <= 5 && t.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    // Docker / proxy style: `8080:80`, `127.0.0.1:8080:80/tcp`
    let base = t
        .strip_suffix("/tcp")
        .or_else(|| t.strip_suffix("/udp"))
        .unwrap_or(t);
    if base.contains(':')
        && base
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b':' || b == b'.' || b == b'-')
    {
        return true;
    }
    false
}

fn redact_header_value(raw_hdr: &str) -> Option<String> {
    let trimmed_hdr = raw_hdr.trim_matches(['"', '\'']);
    let lower_hdr = trimmed_hdr.to_ascii_lowercase();
    if lower_hdr.contains("authorization:")
        || lower_hdr.contains("bearer ")
        || lower_hdr.contains("x-api-key")
        || lower_hdr.contains("api-key:")
        || lower_hdr.contains("x-auth-token")
        || lower_hdr.contains("cookie:")
        || lower_hdr.contains("token")
    {
        let masked = trimmed_hdr
            .split_once(':')
            .map(|(k, _)| format!("{k}: ***"))
            .unwrap_or_else(|| "***".to_string());
        Some(masked)
    } else {
        None
    }
}

fn redact_basic_auth_value(val: &str) -> String {
    let trimmed = val.trim_matches(['"', '\'']);
    if let Some((user, _)) = trimmed.split_once(':') {
        format!("{user}:***")
    } else {
        "***".to_string()
    }
}

/// Redacts secrets such as `--token`, `--password`, `--secret`, `--api-key`,
/// `KEY=VAL` secrets, `-H Authorization: ...`, `Bearer`, Basic Auth, and URL credentials/query secrets.
pub fn sanitize_cmdline(cmdline: &str) -> String {
    let tokens = split_cmdline_preserving_quotes(cmdline);
    let exe = exe_basename_lower(&tokens);
    let mut out = Vec::with_capacity(tokens.len());
    let mut redact_next = false;
    let mut redact_basic_auth_next = false;
    let mut redact_header_next = false;

    for (idx, tok) in tokens.iter().enumerate() {
        if redact_next {
            out.push("***".to_string());
            redact_next = false;
            continue;
        }
        if redact_basic_auth_next {
            out.push(redact_basic_auth_value(tok));
            redact_basic_auth_next = false;
            continue;
        }
        if redact_header_next {
            redact_header_next = false;
            if let Some(masked) = redact_header_value(tok) {
                out.push(masked);
                continue;
            }
        }

        let trimmed = tok.trim_matches(['"', '\'']);
        let lower = trimmed.to_ascii_lowercase();

        if matches!(lower.as_str(), "-h" | "--header") {
            out.push(tok.clone());
            redact_header_next = true;
            continue;
        }

        // Single-token header forms: `--header="Authorization: Bearer ..."` or `-H"Authorization: ..."`
        if let Some(rest) = trimmed
            .strip_prefix("--header=")
            .or_else(|| trimmed.strip_prefix("--HEADER="))
        {
            if let Some(masked) = redact_header_value(rest) {
                out.push(format!("--header={masked}"));
                continue;
            }
        } else if trimmed.starts_with("-H") && trimmed.len() > 2 {
            let rest = &trimmed[2..];
            if let Some(masked) = redact_header_value(rest) {
                out.push(format!("-H{masked}"));
                continue;
            }
        }

        if lower == "bearer" {
            out.push(tok.clone());
            redact_next = true;
            continue;
        }

        // Basic Auth flags: `--user`, `--proxy-user`, and `-u` for curl
        if matches!(lower.as_str(), "--user" | "--proxy-user")
            || (lower == "-u" && exe == "curl")
        {
            out.push(tok.clone());
            redact_basic_auth_next = true;
            continue;
        }
        if let Some((flag, val)) = lower.split_once('=') {
            if matches!(flag, "--user" | "--proxy-user") {
                let raw_flag = tok.split_once('=').map(|(f, _)| f).unwrap_or(tok);
                out.push(format!("{raw_flag}={}", redact_basic_auth_value(val)));
                continue;
            }
        }

        // Redis `-a <password>`
        if lower == "-a" && exe == "redis-cli" {
            out.push(tok.clone());
            redact_next = true;
            continue;
        }

        // Context-aware `-p` password handling (never mask port flags like `next dev -p 3001`)
        if is_password_dash_p_exe(&exe) {
            if lower == "-p" {
                let next_is_port = tokens.get(idx + 1).is_some_and(|n| looks_like_port_spec(n));
                out.push(tok.clone());
                if !next_is_port {
                    redact_next = true;
                }
                continue;
            }
            if trimmed.starts_with("-p") && trimmed.len() > 2 && !looks_like_port_spec(&trimmed[2..]) {
                out.push("-p***".to_string());
                continue;
            }
        }

        if let Some((flag, _val)) = lower.split_once('=') {
            if !flag.contains("://") && (is_sensitive_flag(flag) || is_sensitive_env_key(flag)) {
                let raw_flag = tok.split_once('=').map(|(f, _)| f).unwrap_or(tok);
                out.push(format!("{raw_flag}=***"));
                continue;
            }
        }

        if is_sensitive_flag(&lower)
            || (lower.starts_with("--") && is_sensitive_env_key(&lower))
        {
            out.push(tok.clone());
            redact_next = true;
            continue;
        }

        out.push(redact_url_secrets(tok));
    }

    out.join(" ")
}

fn is_sensitive_env_key(key: &str) -> bool {
    let k = key.trim_start_matches('-').replace('-', "_");
    !k.is_empty()
        && (k.contains("token")
            || k.contains("secret")
            || k.contains("password")
            || k.contains("passwd")
            || k.ends_with("_pwd")
            || k.starts_with("pwd_")
            || k.contains("apikey")
            || k.contains("api_key")
            || k.contains("auth")
            || k.contains("private_key")
            || k.contains("access_key")
            || k.contains("dsn")
            || k.contains("connection_string")
            || k.contains("credential"))
}

pub(super) fn is_sensitive_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--token"
            | "--auth-token"
            | "--access-token"
            | "--password"
            | "--pass"
            | "--secret"
            | "--client-secret"
            | "--api-key"
            | "--apikey"
            | "--bearer"
            | "--dsn"
            | "--connection-string"
    )
}

fn is_sensitive_query_param(key: &str) -> bool {
    let k = key.to_ascii_lowercase().replace('-', "_");
    matches!(
        k.as_str(),
        "token"
            | "access_token"
            | "auth_token"
            | "api_key"
            | "apikey"
            | "key"
            | "secret"
            | "password"
            | "passwd"
            | "auth"
            | "sig"
            | "signature"
    ) || is_sensitive_env_key(&k)
}

pub(super) fn redact_url_secrets(tok: &str) -> String {
    let with_userinfo = redact_url_userinfo(tok);
    let Some(q_pos) = with_userinfo.find('?') else {
        return with_userinfo;
    };
    if !with_userinfo[..q_pos].contains("://") {
        return with_userinfo;
    }
    let prefix = &with_userinfo[..=q_pos];
    let rest = &with_userinfo[q_pos + 1..];
    let (query, frag) = match rest.find('#') {
        Some(f) => (&rest[..f], &rest[f..]),
        None => (rest, ""),
    };
    let mut changed = false;
    let parts: Vec<String> = query
        .split('&')
        .map(|pair| {
            if let Some((k, v)) = pair.split_once('=') {
                if !v.is_empty() && is_sensitive_query_param(k) {
                    changed = true;
                    return format!("{k}=***");
                }
            }
            pair.to_string()
        })
        .collect();
    if changed {
        format!("{prefix}{}{frag}", parts.join("&"))
    } else {
        with_userinfo
    }
}

pub(super) fn redact_url_userinfo(tok: &str) -> String {
    let Some(scheme_end) = tok.find("://") else {
        return tok.to_string();
    };
    let after_scheme = &tok[scheme_end + 3..];
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    let Some(at_pos) = authority.rfind('@') else {
        return tok.to_string();
    };
    let userinfo = &authority[..at_pos];
    let suffix_start = scheme_end + 3 + at_pos;
    if let Some(colon_pos) = userinfo.find(':') {
        let prefix_len = scheme_end + 3 + colon_pos + 1;
        format!("{}***{}", &tok[..prefix_len], &tok[suffix_start..])
    } else {
        let prefix_len = scheme_end + 3;
        format!("{}***{}", &tok[..prefix_len], &tok[suffix_start..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_tokens_headers_env_assignments_and_url_userinfo() {
        assert_eq!(
            strip_shell_redirects("python bot.py --port 8080 >bot.log 2>&1"),
            "python bot.py --port 8080"
        );
        assert_eq!(
            sanitize_cmdline("node app.js --token 'secret_123' API_KEY=xyz"),
            "node app.js --token *** API_KEY=***"
        );
        assert_eq!(
            sanitize_cmdline("curl -H \"Authorization: Bearer tok_999\" https://ghp_secret@github.com/org/repo.git"),
            "curl -H Authorization: *** https://***@github.com/org/repo.git"
        );
        assert_eq!(
            sanitize_cmdline("psql postgres://admin:s3cr3t@localhost:5432/app"),
            "psql postgres://admin:***@localhost:5432/app"
        );
    }

    #[test]
    fn preserves_port_dash_p_flags_and_redacts_mysql_password() {
        assert_eq!(sanitize_cmdline("next dev -p 3001"), "next dev -p 3001");
        assert_eq!(
            sanitize_cmdline("docker run -p 8080:80 nginx"),
            "docker run -p 8080:80 nginx"
        );
        assert_eq!(
            sanitize_cmdline("psql -h localhost -p 5432 -U postgres"),
            "psql -h localhost -p 5432 -U postgres"
        );
        assert_eq!(
            sanitize_cmdline("mysql -u root -pmypass app"),
            "mysql -u root -p*** app"
        );
        assert_eq!(
            sanitize_cmdline("mysql -u root -p mypass app"),
            "mysql -u root -p *** app"
        );
    }

    #[test]
    fn redacts_compound_flags_headers_basic_auth_redis_and_url_queries() {
        assert_eq!(
            sanitize_cmdline("app --github-token ghp_123 --openai-api-key=sk-123 --db-password s3cr3t"),
            "app --github-token *** --openai-api-key=*** --db-password ***"
        );
        assert_eq!(
            sanitize_cmdline("curl --header=\"Authorization: Bearer tok_999\" -H\"Api-Key: sk_123\" -u admin:s3cr3t https://api.example.com/v1?api_key=sk_999&mode=fast&token=eyJ123"),
            "curl --header=Authorization: *** -HApi-Key: *** -u admin:*** https://api.example.com/v1?api_key=***&mode=fast&token=***"
        );
        assert_eq!(
            sanitize_cmdline("redis-cli -h 127.0.0.1 -a supersecret ping"),
            "redis-cli -h 127.0.0.1 -a *** ping"
        );
        assert_eq!(
            sanitize_cmdline("node app.js --api-key 'sk-secret part2' PWD=/home/dev"),
            "node app.js --api-key *** PWD=/home/dev"
        );
    }
}
