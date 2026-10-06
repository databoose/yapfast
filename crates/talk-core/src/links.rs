//! Web addresses in message text become links the reader can click.
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
/// other programs.
pub fn openable(link: &str) -> bool {
    let lower = link.to_ascii_lowercase();
    let host = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    host.is_some_and(|host| !host.is_empty()) && !link.chars().any(char::is_control)
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
        loop {
            let url = &text[start..end];
            let Some(last) = url.chars().next_back() else {
                break;
            };
            let unbalanced = |open: char, close: char| {
                last == close && url.matches(open).count() < url.matches(close).count()
            };
            if matches!(
                last,
                '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '*' | '~' | '|'
            ) || unbalanced('(', ')')
                || unbalanced('[', ']')
            {
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
    fn opens_only_web_pages() {
        assert!(openable("https://example.com"));
        assert!(openable("HTTP://example.com"));
        assert!(!openable("file:///etc/passwd"));
        assert!(!openable("javascript:alert(1)"));
        assert!(!openable("https://"));
    }
}
