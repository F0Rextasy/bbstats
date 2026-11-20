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

/// Redacts secrets such as `--token`, `--password`, `--secret`, `--api-key`,
/// `KEY=VAL` secrets, `-H Authorization: ...`, `Bearer`, and URL credentials.
pub fn sanitize_cmdline(cmdline: &str) -> String {
    let tokens = split_cmdline_preserving_quotes(cmdline);
    let mut out = Vec::with_capacity(tokens.len());
    let mut redact_next = false;
    let mut redact_header_next = false;

    for tok in tokens {
        if redact_next {
            out.push("***".to_string());
            redact_next = false;
            continue;
        }
        if redact_header_next {
            redact_header_next = false;
            let trimmed_hdr = tok.trim_matches(['"', '\'']);
            let lower_hdr = trimmed_hdr.to_ascii_lowercase();
            if lower_hdr.contains("authorization:")
                || lower_hdr.contains("bearer ")
                || lower_hdr.contains("x-api-key")
                || lower_hdr.contains("x-auth-token")
                || lower_hdr.contains("cookie:")
                || lower_hdr.contains("token")
            {
                let masked = trimmed_hdr
                    .split_once(':')
                    .map(|(k, _)| format!("{k}: ***"))
                    .unwrap_or_else(|| "***".to_string());
                out.push(masked);
                continue;
            }
        }

        let trimmed = tok.trim_matches(['"', '\'']);
        let lower = trimmed.to_ascii_lowercase();

        if matches!(lower.as_str(), "-h" | "--header") {
            out.push(tok);
            redact_header_next = true;
            continue;
        }

        if lower == "bearer" {
            out.push(tok);
            redact_next = true;
            continue;
        }

        if let Some((flag, _val)) = lower.split_once('=') {
            if is_sensitive_flag(flag) || is_sensitive_env_key(flag) {
                let raw_flag = tok.split_once('=').map(|(f, _)| f).unwrap_or(&tok);
                out.push(format!("{raw_flag}=***"));
                continue;
            }
        }

        if is_sensitive_flag(&lower) {
            out.push(tok);
            redact_next = true;
            continue;
        }

        out.push(redact_url_userinfo(&tok));
    }

    out.join(" ")
}

fn is_sensitive_env_key(key: &str) -> bool {
    let k = key.trim_start_matches('-');
    !k.is_empty()
        && (k.contains("token")
            || k.contains("secret")
            || k.contains("password")
            || k.contains("passwd")
            || k.contains("pwd")
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
            | "-p"
            | "--secret"
            | "--client-secret"
            | "--api-key"
            | "--apikey"
            | "--bearer"
            | "--dsn"
            | "--connection-string"
    )
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
}

