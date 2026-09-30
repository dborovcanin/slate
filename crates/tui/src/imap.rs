use app_core::config::{resolve_email_note_id, ImapConfig, SpecialNotesConfig};
use app_core::data_dir;
use app_core::storage::Db;
use mailparse::MailHeaderMap;
use regex::Regex;
use rustc_hash::FxHashSet;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::OnceLock;
use time::{Date, Duration, Month, OffsetDateTime};

const MAX_IMAP_LINE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct SyncSummary {
    pub fetched: usize,
    pub appended: usize,
    pub duplicates: usize,
    pub body_truncated: usize,
    pub message_truncated: usize,
}

pub fn run_imap_sync(imap: ImapConfig, special: SpecialNotesConfig) -> Result<SyncSummary, String> {
    run_imap_sync_with_verbosity(imap, special, None, true)
}

pub fn run_imap_sync_silent(
    imap: ImapConfig,
    special: SpecialNotesConfig,
    db: &Db,
) -> Result<SyncSummary, String> {
    run_imap_sync_with_verbosity(imap, special, Some(db), false)
}

fn run_imap_sync_with_verbosity(
    imap: ImapConfig,
    special: SpecialNotesConfig,
    shared_db: Option<&Db>,
    verbose: bool,
) -> Result<SyncSummary, String> {
    imap.validate_runtime()?;

    let password = std::env::var(imap.password_env.trim())
        .map_err(|_| format!("Missing IMAP password env var '{}'", imap.password_env))?;
    if password.trim().is_empty() {
        return Err(format!(
            "IMAP password env var '{}' is empty",
            imap.password_env
        ));
    }

    let owned_db;
    let db = match shared_db {
        Some(db) => db,
        None => {
            let db_path = data_dir()?.join("notes.db");
            owned_db = Db::open(db_path)?;
            &owned_db
        }
    };
    let source_key = build_source_key(&imap);

    let summary = sync_once(db, &imap, &special, &source_key, &password, verbose)?;
    if verbose {
        println!(
            "IMAP sync done: fetched={} appended={} duplicates={} body_truncated={} message_truncated={}",
            summary.fetched,
            summary.appended,
            summary.duplicates,
            summary.body_truncated,
            summary.message_truncated
        );
    }
    Ok(summary)
}

fn sync_once(
    db: &Db,
    imap: &ImapConfig,
    special: &SpecialNotesConfig,
    source_key: &str,
    password: &str,
    verbose: bool,
) -> Result<SyncSummary, String> {
    let mut client = ImapClient::connect(imap)?;
    client.login(&imap.username, password)?;
    let uid_next = client.select(&imap.folder)?;

    let last_uid = db.get_ingest_offset(source_key)?.unwrap_or(0);
    let start_uid = determine_start_uid(last_uid, uid_next, imap.initial_sync_max_messages as u64);
    let sync_since =
        determine_sync_since_date(imap.initial_sync_past_days, OffsetDateTime::now_utc());
    if verbose {
        if let Some(since) = sync_since {
            println!(
                "IMAP sync start: folder={} checkpoint_uid={} start_uid={} since={}",
                imap.folder,
                last_uid,
                start_uid,
                format_imap_search_date(since)
            );
        } else {
            println!(
                "IMAP sync start: folder={} checkpoint_uid={} start_uid={}",
                imap.folder, last_uid, start_uid
            );
        }
    }
    let mut uids = client.search_uids(start_uid, sync_since)?;
    // We prepend each message block into the note, so ingest oldest->newest
    // to keep the newest message at the very top after every sync cycle.
    sort_uids_oldest_first(&mut uids);

    let mut summary = SyncSummary::default();
    let mut max_processed_uid = last_uid;

    for uid in uids {
        summary.fetched += 1;
        let raw = client.fetch_rfc822(uid)?;
        let outcome = ingest_message(db, special, source_key, &raw, imap)?;
        match outcome {
            IngestOutcome::Appended {
                body_truncated,
                message_truncated,
            } => {
                summary.appended += 1;
                if body_truncated {
                    summary.body_truncated += 1;
                }
                if message_truncated {
                    summary.message_truncated += 1;
                }
            }
            IngestOutcome::Duplicate => {
                summary.duplicates += 1;
            }
        }

        if uid as i64 > max_processed_uid {
            max_processed_uid = uid as i64;
        }
    }

    if max_processed_uid > last_uid {
        db.set_ingest_offset(source_key, max_processed_uid)?;
    }

    let _ = client.logout();
    Ok(summary)
}

fn determine_start_uid(
    last_uid: i64,
    uid_next: Option<u64>,
    initial_sync_max_messages: u64,
) -> u64 {
    if last_uid >= 1 {
        return (last_uid as u64).saturating_add(1);
    }

    let window = initial_sync_max_messages.max(1);
    match uid_next {
        Some(next) if next > 1 => next.saturating_sub(window).max(1),
        _ => 1,
    }
}

fn determine_sync_since_date(sync_past_days: u16, now_utc: OffsetDateTime) -> Option<Date> {
    if sync_past_days == 0 {
        return None;
    }
    Some((now_utc - Duration::days(i64::from(sync_past_days))).date())
}

fn sort_uids_oldest_first(uids: &mut [u64]) {
    uids.sort_unstable();
}

fn build_source_key(imap: &ImapConfig) -> String {
    format!(
        "imap:{}:{}:{}",
        imap.host.trim().to_lowercase(),
        imap.username.trim().to_lowercase(),
        imap.folder.trim().to_lowercase()
    )
}

#[derive(Debug)]
enum IngestOutcome {
    Appended {
        body_truncated: bool,
        message_truncated: bool,
    },
    Duplicate,
}

fn ingest_message(
    db: &Db,
    special: &SpecialNotesConfig,
    source_key: &str,
    raw: &[u8],
    imap: &ImapConfig,
) -> Result<IngestOutcome, String> {
    let parsed = mailparse::parse_mail(raw).ok();

    let subject = parsed
        .as_ref()
        .map(|mail| sanitize_subject(mail.headers.get_first_value("Subject")))
        .unwrap_or_else(|| "(unparsed message)".to_string());

    let message_id = parsed
        .as_ref()
        .and_then(|mail| normalize_message_id(mail.headers.get_first_value("Message-ID")));

    let date = parsed
        .as_ref()
        .and_then(|mail| mail.headers.get_first_value("Date"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let from = parsed
        .as_ref()
        .map(|mail| extract_emails(&mail.headers.get_first_value("From").unwrap_or_default()))
        .unwrap_or_default();
    let to = parsed
        .as_ref()
        .map(|mail| extract_emails(&mail.headers.get_first_value("To").unwrap_or_default()))
        .unwrap_or_default();
    let cc = parsed
        .as_ref()
        .map(|mail| extract_emails(&mail.headers.get_first_value("Cc").unwrap_or_default()))
        .unwrap_or_default();

    let attachments = parsed
        .as_ref()
        .map(collect_attachment_names)
        .unwrap_or_default();

    let mut body = if let Some(parsed) = parsed.as_ref() {
        extract_best_body_text(parsed).unwrap_or_else(|| String::from_utf8_lossy(raw).to_string())
    } else {
        String::from_utf8_lossy(raw).to_string()
    }
    .replace("\r\n", "\n");

    let mut body_truncated = false;
    if body.len() > imap.max_body_bytes {
        body = truncate_utf8_to_bytes(&body, imap.max_body_bytes);
        body.push_str("\n\n[message body truncated]");
        body_truncated = true;
    }

    let message_truncated = raw.len() > imap.max_message_bytes;
    let raw_payload = if message_truncated {
        &raw[..imap.max_message_bytes]
    } else {
        raw
    };

    let markdown = build_email_markdown(
        &subject,
        &from,
        &to,
        &cc,
        date.as_deref(),
        message_id.as_deref(),
        &attachments,
        &body,
    );

    let now = OffsetDateTime::now_utc();
    let note_id = resolve_email_note_id(special, now, crate::terminal::local_offset_at(now));
    let appended = db.prepend_note_with_ingest_event(
        source_key,
        message_id.as_deref(),
        &note_id,
        &markdown,
        raw_payload,
        body_truncated,
        message_truncated,
    )?;

    if appended.is_some() {
        Ok(IngestOutcome::Appended {
            body_truncated,
            message_truncated,
        })
    } else {
        Ok(IngestOutcome::Duplicate)
    }
}

struct ImapClient {
    io: StreamOwned<ClientConnection, TcpStream>,
    next_tag: u32,
    max_literal_bytes: usize,
}

#[derive(Debug, Clone)]
struct ImapResponsePart {
    line: String,
    literal: Option<Vec<u8>>,
}

impl ImapClient {
    fn connect(imap: &ImapConfig) -> Result<Self, String> {
        let addr = format!("{}:{}", imap.host, imap.port);
        let tcp = TcpStream::connect(&addr)
            .map_err(|e| format!("Failed to connect to IMAP server {addr}: {e}"))?;
        tcp.set_nodelay(true)
            .map_err(|e| format!("Failed to configure IMAP socket: {e}"))?;

        let roots_result = rustls_native_certs::load_native_certs();
        let mut roots = RootCertStore::empty();
        let (added, _ignored) = roots.add_parsable_certificates(roots_result.certs);
        if added == 0 {
            return Err("Failed to load any native TLS root certificates".to_string());
        }

        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        let server_name = ServerName::try_from(imap.host.to_string())
            .map_err(|_| format!("Invalid IMAP host for TLS SNI: {}", imap.host))?;
        let connection = ClientConnection::new(Arc::new(config), server_name)
            .map_err(|e| format!("Failed to initialize IMAP TLS session: {e}"))?;

        let mut client = Self {
            io: StreamOwned::new(connection, tcp),
            next_tag: 1,
            max_literal_bytes: imap.max_message_bytes,
        };
        let greeting = read_imap_line(&mut client.io, MAX_IMAP_LINE_BYTES)?
            .ok_or_else(|| "IMAP server closed connection before greeting".to_string())?;
        if !greeting.to_ascii_uppercase().starts_with("* OK") {
            return Err(format!("IMAP server did not send OK greeting: {greeting}"));
        }
        Ok(client)
    }

    fn login(&mut self, username: &str, password: &str) -> Result<(), String> {
        let command = format!(
            "LOGIN {} {}",
            quote_imap_string(username),
            quote_imap_string(password)
        );
        self.run_command(&command, "LOGIN *** ***").map(|_| ())
    }

    fn select(&mut self, folder: &str) -> Result<Option<u64>, String> {
        let command = format!("SELECT {}", quote_imap_string(folder));
        let parts = self.run_command(&command, &command)?;
        Ok(parse_uid_next(&parts))
    }

    fn search_uids(&mut self, start_uid: u64, since: Option<Date>) -> Result<Vec<u64>, String> {
        let command = build_uid_search_command(start_uid, since);
        let parts = self.run_command(&command, &command)?;
        let mut out = Vec::new();
        for part in parts {
            let upper = part.line.to_ascii_uppercase();
            if !upper.starts_with("* SEARCH") {
                continue;
            }
            let remainder = part.line[8..].trim();
            for token in remainder.split_whitespace() {
                if let Ok(uid) = token.parse::<u64>() {
                    out.push(uid);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }

    fn fetch_rfc822(&mut self, uid: u64) -> Result<Vec<u8>, String> {
        let command = format!("UID FETCH {} (RFC822)", uid);
        let parts = self.run_command(&command, &command)?;
        for part in parts {
            if let Some(literal) = part.literal {
                return Ok(literal);
            }
        }
        Err(format!(
            "IMAP UID FETCH returned no message payload for UID {uid}"
        ))
    }

    fn logout(&mut self) -> Result<(), String> {
        self.run_command("LOGOUT", "LOGOUT").map(|_| ())
    }

    fn run_command(
        &mut self,
        command: &str,
        redacted_label: &str,
    ) -> Result<Vec<ImapResponsePart>, String> {
        let tag = format!("A{:04}", self.next_tag);
        self.next_tag += 1;

        self.io
            .write_all(format!("{tag} {command}\r\n").as_bytes())
            .map_err(|e| format!("IMAP write failed: {e}"))?;
        self.io
            .flush()
            .map_err(|e| format!("IMAP flush failed: {e}"))?;

        let (parts, tagged_line) =
            read_tagged_response(&mut self.io, &tag, self.max_literal_bytes)?;
        let status = tagged_line[tag.len()..].trim_start();
        if !status.to_ascii_uppercase().starts_with("OK") {
            return Err(format!(
                "IMAP command failed [{redacted_label}]: {tagged_line}"
            ));
        }
        Ok(parts)
    }
}

fn read_tagged_response(
    io: &mut (impl Read + Write),
    tag: &str,
    max_literal_bytes: usize,
) -> Result<(Vec<ImapResponsePart>, String), String> {
    let mut parts = Vec::new();
    loop {
        let line = read_imap_line(io, MAX_IMAP_LINE_BYTES)?
            .ok_or_else(|| "IMAP server closed connection while reading response".to_string())?;

        let literal_len = parse_imap_literal_size(&line);
        let literal = if let Some(literal_len) = literal_len {
            // Keep one byte beyond the configured limit so the ingest path can
            // preserve its existing `message_truncated` signal. Discard the
            // rest in a fixed-size buffer instead of trusting the server's
            // announced literal length for an allocation.
            let retained_len = literal_len.min(max_literal_bytes.saturating_add(1));
            let mut buf = vec![0u8; retained_len];
            io.read_exact(&mut buf)
                .map_err(|e| format!("Failed to read IMAP literal: {e}"))?;
            discard_imap_literal_bytes(io, literal_len.saturating_sub(retained_len))?;
            Some(buf)
        } else {
            None
        };

        if line.starts_with(tag)
            && line
                .as_bytes()
                .get(tag.len())
                .map(|b| *b == b' ')
                .unwrap_or(false)
        {
            return Ok((parts, line));
        }

        parts.push(ImapResponsePart { line, literal });
    }
}

fn discard_imap_literal_bytes(io: &mut impl Read, mut remaining: usize) -> Result<(), String> {
    let mut discard = [0u8; 8192];
    while remaining > 0 {
        let chunk_len = remaining.min(discard.len());
        io.read_exact(&mut discard[..chunk_len])
            .map_err(|e| format!("Failed to discard oversized IMAP literal: {e}"))?;
        remaining -= chunk_len;
    }
    Ok(())
}

fn read_imap_line(io: &mut impl Read, max_bytes: usize) -> Result<Option<String>, String> {
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        let read = io
            .read(&mut byte)
            .map_err(|e| format!("IMAP read failed: {e}"))?;
        if read == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        if byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            buf.push(byte[0]);
            if buf.len() > max_bytes {
                return Err(format!(
                    "IMAP response line exceeded max length (>{max_bytes} bytes)"
                ));
            }
        }
    }
    Ok(Some(String::from_utf8_lossy(&buf).to_string()))
}

fn parse_imap_literal_size(line: &str) -> Option<usize> {
    let line = line.trim_end();
    let start = line.rfind('{')?;
    if !line.ends_with('}') || start + 2 > line.len() {
        return None;
    }
    let mut token = &line[start + 1..line.len() - 1];
    if let Some(stripped) = token.strip_suffix('+') {
        token = stripped;
    }
    token.parse::<usize>().ok()
}

fn parse_uid_next(parts: &[ImapResponsePart]) -> Option<u64> {
    const MARKER: &str = "[UIDNEXT ";
    for part in parts {
        let upper = part.line.to_ascii_uppercase();
        let Some(start) = upper.find(MARKER) else {
            continue;
        };
        let tail = &part.line[start + MARKER.len()..];
        let digits_len = tail.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits_len == 0 {
            continue;
        }
        if let Ok(uid_next) = tail[..digits_len].parse::<u64>() {
            return Some(uid_next);
        }
    }
    None
}

fn build_uid_search_command(start_uid: u64, since: Option<Date>) -> String {
    match since {
        Some(date) => format!(
            "UID SEARCH UID {}:* SINCE {}",
            start_uid,
            format_imap_search_date(date)
        ),
        None => format!("UID SEARCH UID {}:*", start_uid),
    }
}

fn format_imap_search_date(date: Date) -> String {
    let month = match date.month() {
        Month::January => "Jan",
        Month::February => "Feb",
        Month::March => "Mar",
        Month::April => "Apr",
        Month::May => "May",
        Month::June => "Jun",
        Month::July => "Jul",
        Month::August => "Aug",
        Month::September => "Sep",
        Month::October => "Oct",
        Month::November => "Nov",
        Month::December => "Dec",
    };
    format!("{:02}-{}-{:04}", date.day(), month, date.year())
}

fn quote_imap_string(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\r', "")
            .replace('\n', "")
    )
}

fn build_email_markdown(
    subject: &str,
    from: &[String],
    to: &[String],
    cc: &[String],
    date: Option<&str>,
    message_id: Option<&str>,
    attachments: &[String],
    body: &str,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# {}", subject));
    if !from.is_empty() {
        lines.push(format!("from: {}", backticked_list(from)));
    }
    if !to.is_empty() {
        lines.push(format!("to: {}", backticked_list(to)));
    }
    if !cc.is_empty() {
        lines.push(format!("cc: {}", backticked_list(cc)));
    }
    if let Some(date) = date.filter(|value| !value.trim().is_empty()) {
        lines.push(format!("date: {}", date.trim()));
    }
    if let Some(message_id) = message_id.filter(|value| !value.trim().is_empty()) {
        lines.push(format!("message-id: {}", message_id.trim()));
    }
    if !attachments.is_empty() {
        lines.push(format!("attachments: {}", backticked_list(attachments)));
    }
    lines.push(String::new());
    lines.push(body.trim_end().to_string());
    lines.join("\n").trim().to_string()
}

fn backticked_list(values: &[String]) -> String {
    values
        .iter()
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("`{}`", value.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn sanitize_subject(raw: Option<String>) -> String {
    let subject = raw
        .unwrap_or_default()
        .replace('\r', " ")
        .replace('\n', " ")
        .trim()
        .to_string();
    if subject.is_empty() {
        "(no subject)".to_string()
    } else {
        subject
    }
}

fn normalize_message_id(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.replace('\r', "").replace('\n', "").trim().to_string())
        .filter(|value| !value.is_empty())
}

fn extract_emails(raw: &str) -> Vec<String> {
    static EMAIL_RE: OnceLock<Regex> = OnceLock::new();
    let re = EMAIL_RE.get_or_init(|| {
        Regex::new(r"(?i)[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,}").expect("email regex")
    });
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for capture in re.find_iter(raw) {
        let value = capture.as_str().to_lowercase();
        if seen.insert(value.clone()) {
            out.push(value);
        }
    }
    out
}

fn collect_attachment_names(parsed: &mailparse::ParsedMail<'_>) -> Vec<String> {
    let mut names = Vec::new();
    collect_attachment_names_inner(parsed, &mut names);
    dedup_preserve_order(names)
}

fn collect_attachment_names_inner(parsed: &mailparse::ParsedMail<'_>, names: &mut Vec<String>) {
    if parsed.subparts.is_empty() {
        let disposition = parsed.get_content_disposition();
        let disposition_name = format!("{:?}", disposition.disposition).to_ascii_lowercase();
        let filename = disposition
            .params
            .get("filename")
            .cloned()
            .or_else(|| parsed.ctype.params.get("name").cloned());

        let is_attachment = disposition_name == "attachment"
            || disposition.params.contains_key("filename")
            || parsed.ctype.params.contains_key("name");
        if is_attachment {
            if let Some(filename) = filename {
                if !filename.trim().is_empty() {
                    names.push(filename.trim().to_string());
                }
            }
        }
        return;
    }

    for subpart in &parsed.subparts {
        collect_attachment_names_inner(subpart, names);
    }
}

fn extract_best_body_text(parsed: &mailparse::ParsedMail<'_>) -> Option<String> {
    let mut plain = None;
    let mut html = None;
    walk_body(parsed, &mut plain, &mut html);
    plain.or(html)
}

fn walk_body(
    parsed: &mailparse::ParsedMail<'_>,
    plain: &mut Option<String>,
    html: &mut Option<String>,
) {
    if parsed.subparts.is_empty() {
        let mime = parsed.ctype.mimetype.to_ascii_lowercase();
        if mime.starts_with("text/plain") && plain.is_none() {
            if let Ok(body) = parsed.get_body() {
                *plain = Some(body);
            }
        } else if mime.starts_with("text/html") && html.is_none() {
            if let Ok(body) = parsed.get_body() {
                *html = Some(strip_html_tags(&body));
            }
        }
        return;
    }

    for subpart in &parsed.subparts {
        walk_body(subpart, plain, html);
    }
}

fn strip_html_tags(html: &str) -> String {
    // These run once per HTML message part during a sync; compiling them per
    // call dominated the actual matching work.
    static BR_RE: OnceLock<Regex> = OnceLock::new();
    static CLOSING_P_RE: OnceLock<Regex> = OnceLock::new();
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    let br_re = BR_RE.get_or_init(|| Regex::new(r"(?i)<\s*br\s*/?\s*>").expect("br regex"));
    let closing_p_re = CLOSING_P_RE.get_or_init(|| Regex::new(r"(?i)</\s*p\s*>").expect("p regex"));
    let tag_re = TAG_RE.get_or_init(|| Regex::new(r"(?is)<[^>]+>").expect("tag regex"));

    let normalized = html.replace("\r\n", "\n");
    let normalized = br_re.replace_all(&normalized, "\n");
    let normalized = closing_p_re.replace_all(&normalized, "\n\n");
    let normalized = tag_re.replace_all(&normalized, "");
    normalized
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_string()
}

fn dedup_preserve_order(values: Vec<String>) -> Vec<String> {
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            out.push(value);
        }
    }
    out
}

fn truncate_utf8_to_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_email_markdown_formats_metadata() {
        let markdown = build_email_markdown(
            "Subject line",
            &["sender@example.com".to_string()],
            &["a@example.com".to_string(), "b@example.com".to_string()],
            &["c@example.com".to_string()],
            Some("Fri, 17 Apr 2026 10:00:00 +0200"),
            Some("<abc@id>"),
            &["invoice.pdf".to_string()],
            "- item one\n- item two",
        );
        assert!(markdown.contains("# Subject line"));
        assert!(markdown.contains("from: `sender@example.com`"));
        assert!(markdown.contains("to: `a@example.com`, `b@example.com`"));
        assert!(markdown.contains("cc: `c@example.com`"));
        assert!(markdown.contains("message-id: <abc@id>"));
        assert!(markdown.contains("attachments: `invoice.pdf`"));
        assert!(markdown.contains("- item one"));
    }

    #[test]
    fn extract_emails_pulls_unique_addresses() {
        let emails = extract_emails("A <A@example.com>, a@example.com, B <b@example.org>");
        assert_eq!(
            emails,
            vec!["a@example.com".to_string(), "b@example.org".to_string()]
        );
    }

    #[test]
    fn truncate_utf8_respects_boundaries() {
        let text = "abcčćž";
        let truncated = truncate_utf8_to_bytes(text, 5);
        assert_eq!(truncated, "abcč");
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn parse_literal_size_handles_suffix_form() {
        assert_eq!(
            parse_imap_literal_size("* 23 FETCH (RFC822 {123}"),
            Some(123)
        );
        assert_eq!(
            parse_imap_literal_size("* 23 FETCH (RFC822 {456+}"),
            Some(456)
        );
        assert_eq!(parse_imap_literal_size("* NO LITERAL"), None);
    }

    #[test]
    fn tagged_response_discards_literal_bytes_beyond_limit() {
        let response = b"* 1 FETCH (RFC822 {8}\r\nabcdefgh)\r\nA0001 OK done\r\n".to_vec();
        let mut io = std::io::Cursor::new(response);
        let (parts, tagged) =
            read_tagged_response(&mut io, "A0001", 4).expect("response should parse");

        assert_eq!(tagged, "A0001 OK done");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].literal.as_deref(), Some(b"abcde".as_slice()));
        assert_eq!(parts[1].line, ")");
    }

    #[test]
    fn parse_uid_next_reads_select_response() {
        let parts = vec![ImapResponsePart {
            line: "* OK [UIDNEXT 4096] Predicted next UID".to_string(),
            literal: None,
        }];
        assert_eq!(parse_uid_next(&parts), Some(4096));
    }

    #[test]
    fn determine_start_uid_uses_recent_window_on_first_sync() {
        assert_eq!(determine_start_uid(0, Some(101), 50), 51);
        assert_eq!(determine_start_uid(0, Some(10), 50), 1);
        assert_eq!(determine_start_uid(77, Some(101), 50), 78);
    }

    #[test]
    fn determine_sync_since_date_applies_to_every_sync_when_enabled() {
        let now = OffsetDateTime::from_unix_timestamp(1_714_516_200).expect("fixed ts");
        let expected = Date::from_calendar_date(2024, Month::April, 29).expect("date");

        assert_eq!(determine_sync_since_date(1, now), Some(expected));
        assert_eq!(determine_sync_since_date(0, now), None);
    }

    #[test]
    fn build_uid_search_command_includes_optional_since_clause() {
        let date = Date::from_calendar_date(2026, Month::April, 17).expect("date");
        assert_eq!(
            build_uid_search_command(101, Some(date)),
            "UID SEARCH UID 101:* SINCE 17-Apr-2026"
        );
        assert_eq!(build_uid_search_command(101, None), "UID SEARCH UID 101:*");
    }

    #[test]
    fn sort_uids_prefers_oldest_messages_first_for_prepend_flow() {
        let mut uids = vec![4, 1, 7, 3];
        sort_uids_oldest_first(&mut uids);
        assert_eq!(uids, vec![1, 3, 4, 7]);
    }
}
