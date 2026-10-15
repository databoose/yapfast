//! Web addresses in message text become links the reader can click.
use std::borrow::Cow;
use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

/// Turns bare web addresses in Markdown into links, leaving code, existing
/// links and HTML alone.
pub fn linkify(markdown: &str) -> String {
    let mut skipped: Vec<Range<usize>> = Vec::new();
    for (event, range) in
        Parser::new_ext(markdown, Options::ENABLE_STRIKETHROUGH).into_offset_iter()
    {
        match event {
            Event::Start(
                Tag::Link { .. } | Tag::Image { .. } | Tag::CodeBlock(_) | Tag::HtmlBlock,
            )
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_) => skipped.push(range),
            _ => {}
        }
    }
    let mut linked = String::with_capacity(markdown.len() + 16);
    let mut copied = 0;
    for url in addresses(markdown) {
        if skipped
            .iter()
            .any(|skip| skip.start < url.end && url.start < skip.end)
        {
            continue;
        }
        linked.push_str(&markdown[copied..url.start]);
        linked.push('<');
        linked.push_str(&markdown[url.clone()]);
        linked.push('>');
        copied = url.end;
    }
    linked.push_str(&markdown[copied..]);
    linked
}

/// Markdown with the HTML in it escaped, so it shows as typed rather than
/// styling the text around it.
pub fn escape_html(markdown: &str) -> Cow<'_, str> {
    let mut html = Parser::new_ext(markdown, Options::ENABLE_STRIKETHROUGH)
        .into_offset_iter()
        .filter(|(event, _)| matches!(event, Event::Html(_) | Event::InlineHtml(_)))
        .peekable();
    if html.peek().is_none() {
        return Cow::Borrowed(markdown);
    }
    let mut escaped = String::with_capacity(markdown.len() + 16);
    let mut copied = 0;
    for (_, range) in html {
        for (at, _) in markdown[range.clone()].match_indices('<') {
            let at = range.start + at;
            if at >= copied {
                escaped.push_str(&markdown[copied..at]);
                escaped.push('\\');
                copied = at;
            }
        }
    }
    escaped.push_str(&markdown[copied..]);
    Cow::Owned(escaped)
}

/// Plain text as Markdown with its web addresses as links, or `None`
/// without any.
pub fn plain_with_links(text: &str) -> Option<String> {
    let urls = addresses(text);
    if urls.is_empty() {
        return None;
    }
    let mut linked = String::with_capacity(text.len() + 16);
    let mut copied = 0;
    for url in urls {
        escape(&text[copied..url.start], &mut linked);
        linked.push('<');
        linked.push_str(&text[url.clone()]);
        linked.push('>');
        copied = url.end;
    }
    escape(&text[copied..], &mut linked);
    Some(linked)
}

/// Whether Talk opens a clicked link: only web pages, never local files or
/// other programs. A name and password before the host, as in
/// `https://discord.com@evil.example`, and invisible or direction-changing
/// characters only serve to make a link look like it goes somewhere else.
pub fn openable(link: &str) -> bool {
    let lower = link.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !authority.is_empty()
        && !authority.contains('@')
        && !link.chars().any(|c| c.is_control() || hidden(c))
}

/// Unicode's format characters that matter in a link: zero-width ones,
/// and those that change the direction text reads in.
pub fn hidden(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// `text` with Markdown's punctuation escaped by backslashes, so it shows
/// as typed.
pub fn escape_markdown(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 8);
    escape(text, &mut escaped);
    escaped
}

fn escape(text: &str, out: &mut String) {
    for character in text.chars() {
        if character.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(character);
    }
}

/// Byte ranges of `http://` and `https://` addresses, without the
/// punctuation that usually follows one in a sentence.
fn addresses(text: &str) -> Vec<Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = lower[from..].find("http") {
        let start = from + offset;
        let scheme = ["https://", "http://"]
            .into_iter()
            .find(|scheme| lower[start..].starts_with(scheme));
        // Inside a word, such as "xhttp://", it's not an address.
        let in_word = text[..start]
            .chars()
            .next_back()
            .is_some_and(|before| before.is_alphanumeric());
        let Some(scheme) = scheme.filter(|_| !in_word) else {
            from = start + 4;
            continue;
        };
        let length = text[start..]
            .find(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"'))
            .unwrap_or(text.len() - start);
        let mut end = start + length;
        // Counted once and kept as the end moves back: counting again for
        // each character let a run of brackets take seconds.
        let count = |wanted: u8| {
            text.as_bytes()[start..end]
                .iter()
                .filter(|&&byte| byte == wanted)
                .count()
        };
        let (mut parentheses, mut brackets) =
            ((count(b'('), count(b')')), (count(b'['), count(b']')));
        while let Some(last) = text[start..end].chars().next_back() {
            if matches!(
                last,
                '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '*' | '~' | '|'
            ) || (last == ')' && parentheses.0 < parentheses.1)
                || (last == ']' && brackets.0 < brackets.1)
            {
                match last {
                    ')' => parentheses.1 -= 1,
                    ']' => brackets.1 -= 1,
                    _ => {}
                }
                end -= last.len_utf8();
            } else {
                break;
            }
        }
        if end > start + scheme.len() {
            found.push(start..end);
        }
        from = end.max(start + scheme.len());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_bare_addresses_only() {
        assert_eq!(
            linkify("see https://example.com/a_b?c=1. and (http://x.org/wiki/A_(b))!"),
            "see <https://example.com/a_b?c=1>. and (<http://x.org/wiki/A_(b)>)!"
        );
        assert_eq!(
            linkify("**https://bold.example** `https://code.example` [site](https://a.example)"),
            "**<https://bold.example>** `https://code.example` [site](https://a.example)"
        );
        assert_eq!(
            linkify("<https://already.example>"),
            "<https://already.example>"
        );
        assert_eq!(
            linkify("https:// xhttps://no.example"),
            "https:// xhttps://no.example"
        );
        assert_eq!(
            plain_with_links("a*b https://e.example/x_y"),
            Some("a\\*b <https://e.example/x_y>".into())
        );
        assert_eq!(plain_with_links("no links"), None);
    }

    #[test]
    fn escapes_html_only() {
        assert_eq!(
            escape_html("<font color=\"#5865f2\">@Admin</font> <u>x</u>"),
            "\\<font color=\"#5865f2\">@Admin\\</font> \\<u>x\\</u>"
        );
        assert!(matches!(
            escape_html("<https://a.example> a < b <3 `<u>` **b**"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn opens_only_web_pages() {
        assert!(openable("https://example.com"));
        assert!(openable("HTTP://example.com"));
        assert!(!openable("file:///etc/passwd"));
        assert!(!openable("javascript:alert(1)"));
        assert!(!openable("https://"));
        assert!(!openable("https://discord.com@evil.example/"));
        assert!(!openable("https://evil.example/\u{202e}moc.drocsid"));
        assert!(!openable("https://disc\u{200b}ord.com"));
        assert!(openable("https://example.com/a@b?c=@d#@e"));
    }

    #[test]
    fn trims_long_bracket_runs_quickly() {
        let text = format!("http://a{}", ")".repeat(100_000));
        let started = std::time::Instant::now();
        assert_eq!(plain_with_links(&text).unwrap().len(), text.len() * 2 - 6);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
