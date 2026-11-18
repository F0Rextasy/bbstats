use super::cmdline::split_cmdline_preserving_quotes;

pub(super) fn strip_shell_redirects(cmdline: &str) -> String {
    let tokens = split_cmdline_preserving_quotes(cmdline);
    let mut kept = Vec::with_capacity(tokens.len());
    for tok in tokens {
        if tok.starts_with(">>")
            || tok.starts_with("1>>")
            || tok.starts_with("2>>")
            || tok == ">"
            || tok == "1>"
            || tok == "2>"
            || tok == "2>&1"
        {
            break;
        }
        kept.push(tok);
    }
    kept.join(" ")
}

/// Redacts secrets such as `--token`, `--password`, `--secret`, `--api-key`, and URL credentials.
pub fn sanitize_cmdline(cmdline: &str) -> String {
    let tokens = split_cmdline_preserving_quotes(cmdline);
    let mut out = Vec::with_capacity(tokens.len());
    let mut redact_next = false;

    for tok in tokens {
        if redact_next {
            out.push("***".to_string());
            redact_next = false;
            continue;
        }

        let lower = tok.to_ascii_lowercase();
        let trimmed = lower.trim_matches('"');

        if let Some((flag, _val)) = trimmed.split_once('=') {
            if is_sensitive_flag(flag) {
                let raw_flag = tok.split_once('=').map(|(f, _)| f).unwrap_or(&tok);
                out.push(format!("{raw_flag}=***"));
                continue;
            }
        }

        if is_sensitive_flag(trimmed) {
            out.push(tok);
            redact_next = true;
            continue;
        }

        out.push(redact_url_userinfo(&tok));
    }

    out.join(" ")
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
    let Some(colon_pos) = userinfo.find(':') else {
        return tok.to_string();
    };

    let prefix_len = scheme_end + 3 + colon_pos + 1;
    let suffix_start = scheme_end + 3 + at_pos;
    format!("{}***{}", &tok[..prefix_len], &tok[suffix_start..])
}

