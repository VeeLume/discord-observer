//! Escaping for user-controlled text rendered as Discord markdown.

/// Escape Discord markdown so user-controlled text (nicknames, display names)
/// renders literally in embeds and messages.
///
/// Neutralises formatting (`*`, `_`, `~`, `` ` ``, `|`), masked links
/// (`[text](url)`), mention/timestamp syntax (`<@id>`, `<t:…>`), headings and
/// quotes. Bare URLs still auto-link; Discord shows those in full, so they
/// can't hide their target the way a masked link can.
///
/// Don't use for plain-text surfaces (autocomplete labels, audit log reasons
/// shown verbatim), where the backslashes would be visible.
pub fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '~' | '`' | '|' | '[' | ']' | '(' | ')' | '<' | '>' | '#' | '-'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape_markdown;

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(escape_markdown("Valerie 123"), "Valerie 123");
    }

    #[test]
    fn masked_link_neutralised() {
        assert_eq!(
            escape_markdown("[Verify](https://evil.tld)"),
            r"\[Verify\]\(https://evil.tld\)"
        );
    }

    #[test]
    fn formatting_and_mentions_escaped() {
        assert_eq!(escape_markdown("**hi**"), r"\*\*hi\*\*");
        assert_eq!(escape_markdown("a_b"), r"a\_b");
        assert_eq!(escape_markdown("||x||"), r"\|\|x\|\|");
        assert_eq!(escape_markdown("<@123>"), r"\<@123\>");
        assert_eq!(escape_markdown(r"back\slash"), r"back\\slash");
    }
}
