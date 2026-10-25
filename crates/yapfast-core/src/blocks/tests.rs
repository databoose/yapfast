use super::*;

/// `markdown`'s blocks as their kind, whether they're quoted, and their text.
fn shown(markdown: &str) -> Vec<(Kind, bool, String)> {
    split(markdown)
        .unwrap_or_default()
        .iter()
        .map(|block| (block.kind, block.quoted, block.text(markdown).into_owned()))
        .collect()
}

fn text(text: &str) -> (Kind, bool, String) {
    (Kind::Text, false, text.into())
}

fn heading(level: u8, text: &str) -> (Kind, bool, String) {
    (Kind::Heading(level), false, text.into())
}

fn quoted((kind, _, text): (Kind, bool, String)) -> (Kind, bool, String) {
    (kind, true, text)
}

#[test]
fn text_alone_has_no_blocks() {
    for markdown in [
        "",
        "hello **there**",
        "a > b",
        "C# and F#",
        "#hashtag",
        "#### four",
        "```open",
        "``````",
        "- a list\n- of two",
        "`` ``` ``",
    ] {
        assert_eq!(split(markdown), None, "{markdown:?}");
    }
}

#[test]
fn headings_take_one_to_three_hashes_and_a_space() {
    assert_eq!(
        shown("# One\n## Two\n### Three\n#### Four"),
        [
            heading(1, "One"),
            heading(2, "Two"),
            heading(3, "Three"),
            text("#### Four")
        ]
    );
    assert_eq!(shown("#\tTab"), [heading(1, "Tab")]);
}

#[test]
fn a_heading_loses_its_closing_hashes_and_spaces() {
    assert_eq!(shown("#  Title ##  "), [heading(1, "Title")]);
    assert_eq!(shown("# C#"), [heading(1, "C")]);
    assert_eq!(shown("# ####"), [heading(1, "#")]);
}

#[test]
fn a_heading_marker_after_a_heading_marker_is_text() {
    assert_eq!(split("# # x"), None);
    assert_eq!(split("# "), None);
}

#[test]
fn spaces_before_a_heading_count_only_where_a_block_could_start() {
    assert_eq!(shown("  # Top"), [heading(1, "Top")]);
    assert_eq!(split("text\n  # not"), None);
    assert_eq!(shown("# a\n  # b"), [heading(1, "a"), heading(1, "b")]);
}

#[test]
fn text_around_blocks_keeps_its_blank_lines_but_not_its_last_break() {
    assert_eq!(
        shown("before\n# Title\nafter"),
        [text("before"), heading(1, "Title"), text("after")]
    );
    assert_eq!(
        shown("a\n\n# T\n\nb"),
        [text("a\n"), heading(1, "T"), text("\nb")]
    );
    assert_eq!(
        shown("# a\n\n# b"),
        [heading(1, "a"), text(""), heading(1, "b")]
    );
}

#[test]
fn subtext_needs_its_space_and_some_text() {
    assert_eq!(
        shown("-# small print\n-#no\n-# \n-# -# twice"),
        [
            (Kind::Subtext, false, "small print".into()),
            text("-#no\n-# \n-# -# twice"),
        ]
    );
}

#[test]
fn quote_lines_run_together_without_their_markers() {
    assert_eq!(
        shown("> one\n>  two\n> \nafter"),
        [quoted(text("one\n two")), text("after")]
    );
}

#[test]
fn a_quote_marker_needs_a_space_and_quotes_dont_nest() {
    assert_eq!(split(">no\n>> no\n>>>no"), None);
    assert_eq!(shown("> > inner"), [quoted(text("> inner"))]);
}

#[test]
fn three_markers_quote_the_rest() {
    assert_eq!(
        shown("before\n>>> one\ntwo\n> three"),
        [text("before"), quoted(text("one\ntwo\n> three"))]
    );
}

#[test]
fn an_empty_quote_still_shows() {
    assert_eq!(shown("> "), [quoted(text(""))]);
}

#[test]
fn a_quote_holds_headings_subtext_and_code() {
    let markdown = "> # Big\n> -# small\n> ```\n> code\n> ```\n> end";
    assert_eq!(
        shown(markdown),
        [
            quoted(heading(1, "Big")),
            quoted((Kind::Subtext, false, "small".into())),
            quoted((Kind::Code, false, "code".into())),
            quoted(text("end")),
        ]
    );
}

#[test]
fn a_block_in_a_quote_knows_where_its_text_was() {
    let markdown = "> a @Bob\n> b";
    let blocks = split(markdown).unwrap();
    let [block] = blocks.as_slice() else {
        panic!("{blocks:?}");
    };
    assert_eq!(block.offset(markdown.find("@Bob").unwrap(), 4), Some(2));
    assert_eq!(block.offset(markdown.rfind('b').unwrap(), 1), Some(7));
    // A marker isn't in its text.
    assert_eq!(block.offset(0, 1), None);
}

#[test]
fn code_is_fenced_anywhere_and_drops_its_language() {
    assert_eq!(
        shown("see ```rust\nfn main() {}\n``` there"),
        [
            text("see "),
            (Kind::Code, false, "fn main() {}".into()),
            text(" there"),
        ]
    );
    // A language needs its own line.
    assert_eq!(shown("```js x```"), [(Kind::Code, false, "js x".into())]);
    assert_eq!(shown("```js\n```"), [(Kind::Code, false, "js".into())]);
}

#[test]
fn code_loses_the_line_breaks_around_it() {
    assert_eq!(
        shown("a\n```\n\ncode\n\n```\nb"),
        [text("a"), (Kind::Code, false, "code".into()), text("b")]
    );
}

#[test]
fn nothing_in_code_is_a_block() {
    assert_eq!(
        shown("```\n# no\n> no\n```"),
        [(Kind::Code, false, "# no\n> no".into())]
    );
    assert_eq!(split("`# no`\n`` ``` ``"), None);
}

#[test]
fn nothing_in_a_spoiler_comes_apart() {
    assert_eq!(split("||```hidden```||"), None);
    assert_eq!(split("||a\n# b||"), None);
}

#[test]
fn an_escaped_backtick_opens_no_code() {
    assert_eq!(split("\\```not code```"), None);
}
