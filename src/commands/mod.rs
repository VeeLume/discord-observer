use anyhow::Result;

use crate::repos::MembershipRow;
use crate::state::Ctx;

pub mod botstats;
pub mod member;
pub mod notice;
pub mod register;
pub mod settings;
pub mod stats;
pub mod userinfo;

pub const MAX_EMBED_DESCRIPTION_CHARS: usize = 4096;

/// Parse an RFC2822 timestamp string into a Unix timestamp (seconds).
pub fn rfc2822_to_unix(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(s)
        .ok()
        .map(|dt| dt.timestamp())
}

/// Format a member label from cached names, with a non-pinging fallback.
/// Priority: server_nickname > display_name > account_username > user_id
pub fn format_member_label(
    user_id: &str,
    account_username: &Option<String>,
    display_name: &Option<String>,
    server_nickname: &Option<String>,
) -> String {
    let best = server_nickname
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| display_name.as_deref().filter(|s| !s.is_empty()))
        .or_else(|| account_username.as_deref());
    match best {
        Some(name) => match account_username.as_deref() {
            Some(acc) if acc != name => format!("{name} (aka {acc})"),
            _ => name.to_string(),
        },
        None => user_id.to_string(),
    }
}

/// Format membership history rows into compact stay lines.
///
/// Each stay becomes one line like:
///   **Stay #1:** <t:123:R> → <t:456:R> (left) · invite `abc`
///   **Stay #2:** <t:789:R> → now
pub fn format_stay_lines(rows: &[MembershipRow]) -> Vec<String> {
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            let num = i + 1;
            let joined = rfc2822_to_unix(&r.joined_at)
                .map(|ts| format!("<t:{ts}:R>"))
                .unwrap_or_else(|| r.joined_at.clone());

            let left = match r.left_at.as_deref() {
                Some(left_str) => {
                    let action = if r.banned {
                        "banned"
                    } else if r.kicked {
                        "kicked"
                    } else {
                        "left"
                    };
                    let ts = rfc2822_to_unix(left_str)
                        .map(|ts| format!("<t:{ts}:R>"))
                        .unwrap_or_else(|| left_str.to_string());
                    format!("{ts} ({action})")
                }
                None => "now".to_string(),
            };

            let invite = r
                .invite_code
                .as_deref()
                .map(|c| format!(" · invite `{c}`"))
                .unwrap_or_default();

            format!("**Stay #{num}:** {joined} → {left}{invite}")
        })
        .collect()
}

/// Split lines into description chunks, each <= max_chars (counted in Unicode scalar values).
pub fn chunk_lines(lines: &[String], max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for line in lines {
        let line_len = line.chars().count();
        // +1 for the newline if current is not empty
        let extra = if current.is_empty() {
            line_len
        } else {
            line_len + 1
        };

        if current_len + extra > max_chars {
            if !current.is_empty() {
                chunks.push(current);
            }
            current = line.clone();
            current_len = line_len;
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
            current_len += extra;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Generic helper:
/// - `lines` → will be joined into descriptions (split into chunks).
/// - `build_first` → called for the first chunk; lets you add thumbnail/fields/etc.
/// - `build_cont`  → called for each continuation chunk with `(index, chunk)`.
pub async fn send_chunked_embeds<BF, BC>(
    ctx: Ctx<'_>,
    lines: Vec<String>,
    build_first: BF,
    build_cont: BC,
) -> Result<()>
where
    BF: FnOnce(String) -> serenity::all::CreateEmbed,
    BC: Fn(usize, String) -> serenity::all::CreateEmbed,
{
    use poise::CreateReply;

    let chunks = chunk_lines(&lines, MAX_EMBED_DESCRIPTION_CHARS);
    if chunks.is_empty() {
        // Caller usually checks, but being defensive.
        return Ok(());
    }

    // First embed
    let first_desc = chunks[0].clone();
    let first_embed = build_first(first_desc);
    ctx.send(CreateReply::default().embed(first_embed)).await?;

    // Continuations
    if chunks.len() > 1 {
        for (idx, chunk) in chunks.into_iter().enumerate().skip(1) {
            let embed = build_cont(idx, chunk);
            ctx.send(CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}
