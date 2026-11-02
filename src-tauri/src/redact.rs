//! Redaction of everything that goes into a log line, a panic message or the diagnostics text
//! (BRAI-368). Pure. Tokens, cookies, auth codes and the query / fragment / opaque path parts
//! of URLs never reach the disk. Written to be idempotent: `redact(redact(x)) == redact(x)`.
//!
//! This is a safety net, not a licence to log secrets: the code does not log them in the first
//! place (`TokenPair` has no `Debug` for its tokens, URLs are logged by origin).

const MASK: &str = "[redacted]";

/// Keys whose value is always secret.
const STRONG_KEYS: &[&str] = &[
    "access_token",
    "refresh_token",
    "id_token",
    "token",
    "code_verifier",
    "verifier",
    "client_secret",
    "secret",
    "password",
    "passwd",
    "api_key",
    "apikey",
    "authorization",
    "proxy-authorization",
    "set-cookie",
    "cookie",
    "xsrf-token",
    "csrf-token",
];

/// Keys that are secret only when the value looks opaque (a state / code / session id), so that
/// `state=Connecting` or `code: 404` stay readable.
const WEAK_KEYS: &[&str] = &["code", "state", "session", "session_id", "sid"];

/// Keys whose value runs to the end of the line (headers: `Cookie: a=b; c=d`).
const LINE_KEYS: &[&str] = &["authorization", "proxy-authorization", "set-cookie", "cookie"];

/// Path segments after which the next segment is a secret (`/app/session/<one-time code>`).
const SECRET_AFTER: &[&str] = &[
    "session", "token", "code", "reset", "invite", "verify", "unsubscribe", "signed", "callback-code",
];

pub fn redact(input: &str) -> String {
    redact_home(&redact_tokens(&redact_pairs(&redact_urls(input))))
}

/// `/Users/alice/…`, `/home/alice/…`, `C:\Users\alice\…` → the user name becomes `~`: build
/// paths in panic locations and file paths must not name the person.
fn redact_home(input: &str) -> String {
    let mut out = input.to_string();
    for marker in ["/Users/", "/home/", "\\Users\\"] {
        let sep = if marker.starts_with('\\') { '\\' } else { '/' };
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(pos) = rest.find(marker) {
            let name_start = pos + marker.len();
            result.push_str(&rest[..name_start]);
            let name_len = rest[name_start..]
                .find(|c: char| c == sep || c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ':'))
                .unwrap_or(rest.len() - name_start);
            if name_len > 0 {
                result.push('~');
            }
            rest = &rest[name_start + name_len..];
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// One log line: redacted, on one line, bounded.
pub fn log_line(input: &str, max_chars: usize) -> String {
    let flat = redact(input).replace("\r\n", "\\n").replace(['\n', '\r'], "\\n");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let mut cut: String = flat.chars().take(max_chars).collect();
    cut.push('…');
    cut
}

fn is_url_end(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | '}' | '`' | '|' | '\\')
}

/// `scheme://user:pass@host:port/path?query#fragment` → `scheme://host:port/path?[redacted]`,
/// with opaque path segments masked too.
fn redact_urls(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find("://") {
        let head = &rest[..pos];
        let scheme_start = head
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            .last()
            .map_or(pos, |(i, _)| i);
        let after = &rest[pos + 3..];
        if scheme_start == pos {
            out.push_str(&rest[..pos + 3]);
            rest = after;
            continue;
        }
        let end = after.find(is_url_end).unwrap_or(after.len());
        out.push_str(&rest[..pos + 3]);
        out.push_str(&sanitize_url_tail(&after[..end]));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn sanitize_url_tail(tail: &str) -> String {
    let (before_fragment, fragment) = match tail.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (tail, None),
    };
    let (location, query) = match before_fragment.split_once('?') {
        Some((l, q)) => (l, Some(q)),
        None => (before_fragment, None),
    };
    let (authority, path) = match location.find('/') {
        Some(i) => location.split_at(i),
        None => (location, ""),
    };
    // user:pass@host → host
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);

    let mut out = String::from(host);
    let mut previous = "";
    for (i, segment) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        if opaque_segment(segment) || (SECRET_AFTER.contains(&previous.to_ascii_lowercase().as_str()) && !segment.is_empty())
        {
            out.push_str(MASK);
        } else {
            out.push_str(segment);
        }
        previous = segment;
    }
    if query.is_some_and(|q| !q.is_empty() && q != MASK) {
        out.push('?');
        out.push_str(MASK);
    } else if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(if f.is_empty() || f == MASK { f } else { MASK });
    }
    out
}

/// A long identifier-like segment with a digit: an id, a code, a signed token.
fn opaque_segment(segment: &str) -> bool {
    segment.len() >= 20
        && segment.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'%'))
        && segment.bytes().any(|b| b.is_ascii_digit())
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// `key=value`, `key: value`, `"key":"value"` for the secret keys; `Bearer <token>`.
fn redact_pairs(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let (s, l) = (input.as_bytes(), lower.as_bytes());
    let mut out = String::with_capacity(input.len());
    let mut last = 0;
    let mut i = 0;
    while i < s.len() {
        let at_word_start = i == 0 || !is_word_byte(l[i - 1]);
        if at_word_start {
            if let Some((value_start, value_end)) = secret_value_at(s, l, i) {
                out.push_str(&input[last..value_start]);
                out.push_str(MASK);
                last = value_end;
                i = value_end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&input[last..]);
    out
}

/// If a secret `key<sep>value` or `bearer value` starts at `i`: the byte range of the value.
fn secret_value_at(s: &[u8], l: &[u8], i: usize) -> Option<(usize, usize)> {
    if l[i..].starts_with(b"bearer ") {
        let start = i + 7;
        let end = run_end(s, start, |b| b.is_ascii_whitespace() || matches!(b, b'"' | b'\'' | b',' | b';'));
        return (end > start && &s[start..end] != MASK.as_bytes()).then_some((start, end));
    }
    let (key, strong) = STRONG_KEYS
        .iter()
        .map(|k| (*k, true))
        .chain(WEAK_KEYS.iter().map(|k| (*k, false)))
        .filter(|(k, _)| l[i..].starts_with(k.as_bytes()))
        .max_by_key(|(k, _)| k.len())?;
    let mut j = i + key.len();
    if j < s.len() && is_word_byte(l[j]) {
        return None; // `tokens`, `codex`, `state-machine`
    }
    if j < s.len() && matches!(s[j], b'"' | b'\'') {
        j += 1; // JSON key: "token": "…"
    }
    while j < s.len() && s[j] == b' ' {
        j += 1;
    }
    if j >= s.len() || !matches!(s[j], b'=' | b':') {
        return None;
    }
    j += 1;
    if j < s.len() && s[j] == b'/' {
        return None; // `state://`
    }
    while j < s.len() && s[j] == b' ' {
        j += 1;
    }
    let quote = (j < s.len() && matches!(s[j], b'"' | b'\'')).then(|| s[j]);
    if quote.is_some() {
        j += 1;
    }
    let start = j;
    let end = match quote {
        Some(q) => run_end(s, start, |b| b == q),
        None if LINE_KEYS.contains(&key) => run_end(s, start, |b| matches!(b, b'\n' | b'\r')),
        None => run_end(s, start, |b| {
            b.is_ascii_whitespace() || matches!(b, b'&' | b'"' | b'\'' | b',' | b';' | b')' | b']' | b'}')
        }),
    };
    let value = &s[start..end];
    if value.is_empty() || value.starts_with(MASK.as_bytes()) {
        return None;
    }
    if !strong && !looks_opaque(value) {
        return None;
    }
    Some((start, end))
}

fn run_end(s: &[u8], start: usize, stop: impl Fn(u8) -> bool) -> usize {
    s[start..].iter().position(|b| stop(*b)).map_or(s.len(), |p| start + p)
}

/// For the weak keys: long and not just a word.
fn looks_opaque(value: &[u8]) -> bool {
    value.len() >= 12 && !value.iter().all(|b| b.is_ascii_lowercase() || matches!(b, b'_' | b'-'))
}

/// Brainitive access (`bra_`) and refresh (`brr_`) tokens and JWTs wherever they appear.
fn redact_tokens(input: &str) -> String {
    let is_token_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '~' | '+' | '/' | '=');
    let mut out = String::with_capacity(input.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if is_secret_token(run) {
            out.push_str("[redacted-token]");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in input.chars() {
        if is_token_char(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn is_secret_token(run: &str) -> bool {
    let body_ok = |prefix: &str| run.len() >= prefix.len() + 8 && run.starts_with(prefix);
    body_ok("bra_") || body_ok("brr_") || (run.starts_with("eyJ") && run.matches('.').count() >= 2 && run.len() >= 20)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS: &str = "bra_0123456789abcdefghijklmnopqrstuv";
    const REFRESH: &str = "brr_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
    const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJl";

    fn no_leak(out: &str, secrets: &[&str]) {
        for s in secrets {
            assert!(!out.contains(s), "{s} leaked in {out}");
        }
    }

    #[test]
    fn app_tokens_and_jwts_are_masked_anywhere() {
        let out = redact(&format!("refresh failed for {REFRESH}, had {ACCESS} and {JWT}."));
        no_leak(&out, &[ACCESS, REFRESH, JWT, "0123456789abcdef"]);
        assert_eq!(out.matches("[redacted-token]").count(), 3);
        assert!(out.starts_with("refresh failed for "));
    }

    #[test]
    fn bearer_and_headers() {
        let out = redact(&format!("Authorization: Bearer {ACCESS}\nAccept: application/json"));
        no_leak(&out, &[ACCESS, "Bearer b"]);
        assert!(out.contains("Accept: application/json"), "{out}");
        let out = redact("sent header bearer abc123secret, retrying");
        no_leak(&out, &["abc123secret"]);
        assert!(out.contains("retrying"));

        let out = redact("Cookie: laravel_session=abc123; XSRF-TOKEN=def456\nnext line");
        no_leak(&out, &["abc123", "def456", "laravel_session"]);
        assert!(out.ends_with("next line"));
        let out = redact("Set-Cookie: sid=zzz999; Path=/; HttpOnly");
        no_leak(&out, &["zzz999"]);
    }

    #[test]
    fn key_value_pairs_in_every_shape() {
        for (input, secret) in [
            ("access_token=SuperSecret99&x=1", "SuperSecret99"),
            ("refresh_token: SuperSecret99", "SuperSecret99"),
            (r#"{"token":"SuperSecret99","a":1}"#, "SuperSecret99"),
            (r#"{"password": "hunter2hunter2"}"#, "hunter2hunter2"),
            ("code_verifier=VerifierVerifierVerifier123", "VerifierVerifier"),
            ("client_secret='s3cr3t value'", "s3cr3t"),
            ("code=AuthCode1234567890abcdef state=StateValue1234567890xyz", "AuthCode1234567890abcdef"),
            ("state=StateValue1234567890xyz", "StateValue1234567890xyz"),
            ("TOKEN=UpperCaseSecret", "UpperCaseSecret"),
        ] {
            let out = redact(input);
            no_leak(&out, &[secret]);
            assert!(out.contains(MASK), "{input} → {out}");
        }
        assert!(redact(r#"{"token":"SuperSecret99","a":1}"#).ends_with(r#""a":1}"#));
    }

    #[test]
    fn harmless_words_survive() {
        for input in [
            "phase=Connecting",
            "state=Connecting",
            "state: unreachable",
            "code: 404",
            "code=503 from server",
            "tokens refreshed",
            "token expired",
            "the session ended",
            "state://localhost",
            "startup: setup done +120ms",
            "opening https://www.brainitive.app/api/v1/instance now",
        ] {
            assert_eq!(redact(input), input, "{input}");
        }
    }

    #[test]
    fn urls_lose_query_fragment_userinfo_and_one_time_codes() {
        let out = redact("GET https://user:pw@brain.example.com:8443/app/authorize?code=abc&state=xyz#frag failed");
        assert_eq!(out, "GET https://brain.example.com:8443/app/authorize?[redacted]#[redacted] failed");

        let out = redact("brainitive://auth/callback?code=AuthCode1234567890&state=S");
        assert_eq!(out, "brainitive://auth/callback?[redacted]");

        let out = redact("navigating to \"https://brain.example.com/app/session/Zm9vYmFy\" ok");
        assert_eq!(out, "navigating to \"https://brain.example.com/app/session/[redacted]\" ok");

        let out = redact("https://x.example/files/3f2c9a1b-77d4-4e8a-9c1d-0123456789ab/download");
        assert_eq!(out, "https://x.example/files/[redacted]/download");

        // a plain URL is kept, a trailing sentence dot too
        assert_eq!(redact("see https://www.brainitive.app/dashboard."), "see https://www.brainitive.app/dashboard.");
        assert_eq!(redact("origin http://localhost:8000"), "origin http://localhost:8000");
        // an empty query / no query stays as is
        assert_eq!(redact("https://a.example/p?"), "https://a.example/p?");
    }

    #[test]
    fn urls_inside_error_chains() {
        let out = redact(
            "error sending request for url (https://brain.example.com/api/v1/instance?token=abcdefgh): dns error",
        );
        assert_eq!(
            out,
            "error sending request for url (https://brain.example.com/api/v1/instance?[redacted]): dns error"
        );
    }

    #[test]
    fn redaction_is_idempotent() {
        for input in [
            format!("Bearer {ACCESS} {REFRESH} {JWT}"),
            "https://u:p@h.example/session/abc?code=1#x".to_string(),
            "Cookie: a=b; c=d".to_string(),
            r#"{"access_token":"abc","code":"abcdefghijklmnop"}"#.to_string(),
            "plain text".to_string(),
        ] {
            let once = redact(&input);
            assert_eq!(redact(&once), once, "{input}");
        }
    }

    #[test]
    fn log_lines_are_single_line_and_bounded() {
        assert_eq!(log_line("a\nb\r\nc", 100), "a\\nb\\nc");
        let long = "x".repeat(50);
        let out = log_line(&long, 10);
        assert_eq!(out.chars().count(), 11);
        assert!(out.ends_with('…'));
        // multi-byte text is cut on a character, not a byte
        assert!(log_line("ééééé", 3).starts_with("ééé"));
        no_leak(&log_line(&format!("token={ACCESS}\nnext"), 200), &[ACCESS]);
    }

    #[test]
    fn home_directories_do_not_name_the_user() {
        assert_eq!(
            redact("panicked at /Users/alice/Projects/app/src/lib.rs:12:5"),
            "panicked at /Users/~/Projects/app/src/lib.rs:12:5"
        );
        assert_eq!(redact("at /home/bob/.cargo/x.rs:1"), "at /home/~/.cargo/x.rs:1");
        assert_eq!(redact(r"C:\Users\Carol\AppData\x.rs"), r"C:\Users\~\AppData\x.rs");
        assert_eq!(redact("/Users/~/a"), "/Users/~/a");
        assert_eq!(redact("/usr/local/lib"), "/usr/local/lib");
    }

    #[test]
    fn non_ascii_text_is_kept() {
        assert_eq!(redact("Zürich — Café ✓ state=Verbunden"), "Zürich — Café ✓ state=Verbunden");
    }
}
