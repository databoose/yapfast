//! Mentioning people and roles from the composer: the `@` word being typed, who
//! it may mean, and the message text that mentions them on its network.

/// Someone or something a message can mention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// What follows the `@` in the composer.
    pub name: String,
    /// How a message says it on its network, such as `<@20>` on Discord.
    pub mention: String,
    /// The address of their avatar; see [`crate::chat::Command::Pictures`].
    pub avatar: Option<String>,
    /// The RGB colour of the name.
    pub color: Option<u32>,
    /// A role or a group such as @everyone, rather than a person.
    pub group: bool,
}

/// Someone or something a message mentions, as the message shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mentioned {
    /// The text in the message, such as `@Bob`.
    pub text: String,
    pub target: Target,
}

/// Who a mention points at: a user, by Discord or Matrix id, or a Discord role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    User(String),
    Role(String),
}

const USER_LINK: &str = "talk:user/";
const ROLE_LINK: &str = "talk:role/";
const MATRIX_TO: &str = "https://matrix.to/#/";

/// The address a mention links to in a message; clicking it opens the person or role.
pub fn link(target: &Target) -> String {
    match target {
        Target::User(id) => format!("{USER_LINK}{id}"),
        Target::Role(id) => format!("{ROLE_LINK}{id}"),
    }
}

/// Who a clicked link mentions: Talk's mention links, and Matrix's links to a user.
pub fn target(link: &str) -> Option<Target> {
    if let Some(id) = link.strip_prefix(USER_LINK) {
        return Some(Target::User(id.to_owned()));
    }
    if let Some(id) = link.strip_prefix(ROLE_LINK) {
        return Some(Target::Role(id.to_owned()));
    }
    let user = link.strip_prefix(MATRIX_TO)?;
    let user = user.split(['?', '/']).next()?;
    let user = user
        .replace("%40", "@")
        .replace("%3A", ":")
        .replace("%3a", ":");
    (user.starts_with('@') && user.contains(':')).then_some(Target::User(user))
}

/// Markdown with each mention's text, outside code, linked to its target.
pub fn linked(markdown: &str, mentions: &[Mentioned]) -> String {
    if mentions.is_empty() {
        return markdown.to_owned();
    }
    let mut sorted: Vec<&Mentioned> = mentions.iter().filter(|m| !m.text.is_empty()).collect();
    sorted.sort_by_key(|mention| std::cmp::Reverse(mention.text.len()));
    let mut out = String::with_capacity(markdown.len() + 32 * mentions.len());
    let mut code = false;
    let mut rest = markdown;
    while let Some(next) = rest.chars().next() {
        if next == '`' {
            code = !code;
        }
        if !code && let Some(mention) = sorted.iter().find(|m| rest.starts_with(&m.text)) {
            let text = mention.text.replace('[', "\\[").replace(']', "\\]");
            out.push_str(&format!("[{text}](<{}>)", link(&mention.target)));
            rest = &rest[mention.text.len()..];
            continue;
        }
        out.push(next);
        rest = &rest[next.len_utf8()..];
    }
    out
}

/// Names longer than this aren't looked up while typing.
const MAX_QUERY: usize = 32;

/// The `@` word ending at `cursor`, a byte offset into `text`: where its `@` is,
/// and what follows it.
pub fn query(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = text.get(..cursor)?;
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    let starts_word = before[..at]
        .chars()
        .next_back()
        .is_none_or(|previous| previous.is_whitespace() || "([{\"'".contains(previous));
    (starts_word && query.len() <= MAX_QUERY && !query.contains(char::is_whitespace))
        .then_some((at, query))
}

/// The candidates `query` may mean, at most `limit`: names that start with it,
/// then names with a later word that does.
pub fn matching<'a>(candidates: &'a [Candidate], query: &str, limit: usize) -> Vec<&'a Candidate> {
    let query = query.to_lowercase();
    let starts = |candidate: &&Candidate| candidate.name.to_lowercase().starts_with(&query);
    let later_word = |candidate: &&Candidate| {
        candidate
            .name
            .to_lowercase()
            .split_whitespace()
            .skip(1)
            .any(|word| word.starts_with(&query))
    };
    let mut found: Vec<&Candidate> = candidates.iter().filter(starts).collect();
    found.extend(candidates.iter().filter(|c| !starts(c) && later_word(c)));
    found.truncate(limit);
    found
}

/// `text` with each `@name` of a candidate, outside code, in its network's form.
/// The longest name that fits wins, so `@Ann Lee` isn't taken for `@Ann`.
pub fn wire(text: &str, candidates: &[Candidate]) -> String {
    let mut sorted: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| !candidate.name.is_empty())
        .collect();
    sorted.sort_by_key(|candidate| std::cmp::Reverse(candidate.name.len()));
    let mut out = String::with_capacity(text.len());
    let mut code = false;
    let mut rest = text;
    let mut previous: Option<char> = None;
    while let Some(next) = rest.chars().next() {
        if next == '`' {
            code = !code;
        }
        let starts_word = previous.is_none_or(|previous| !previous.is_alphanumeric());
        if next == '@' && !code && starts_word {
            let after = &rest[1..];
            let found = sorted.iter().find(|candidate| {
                let name = &candidate.name;
                after
                    .get(..name.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(name))
                    && after[name.len()..]
                        .chars()
                        .next()
                        .is_none_or(|end| !end.is_alphanumeric() && end != '_')
            });
            if let Some(candidate) = found {
                out.push_str(&candidate.mention);
                rest = &after[candidate.name.len()..];
                previous = candidate.name.chars().next_back();
                continue;
            }
        }
        out.push(next);
        rest = &rest[next.len_utf8()..];
        previous = Some(next);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str, mention: &str) -> Candidate {
        Candidate {
            name: name.into(),
            mention: mention.into(),
            avatar: None,
            color: None,
            group: false,
        }
    }

    #[test]
    fn mentions_link_to_who_they_mention() {
        let mentions = [
            Mentioned {
                text: "@Bob".into(),
                target: Target::User("2".into()),
            },
            Mentioned {
                text: "@Mods".into(),
                target: Target::Role("30".into()),
            },
        ];
        assert_eq!(
            linked("hi @Bob, ask @Mods but not `@Bob`", &mentions),
            "hi [@Bob](<talk:user/2>), ask [@Mods](<talk:role/30>) but not `@Bob`"
        );
        assert_eq!(target("talk:user/2"), Some(Target::User("2".into())));
        assert_eq!(target("talk:role/30"), Some(Target::Role("30".into())));
        assert_eq!(
            target("https://matrix.to/#/@sam:test?via=test"),
            Some(Target::User("@sam:test".into()))
        );
        assert_eq!(
            target("https://matrix.to/#/%40sam%3Atest"),
            Some(Target::User("@sam:test".into()))
        );
        assert_eq!(target("https://matrix.to/#/#games:test"), None);
        assert_eq!(target("https://example.com"), None);
    }

    #[test]
    fn finds_the_word_being_typed() {
        assert_eq!(query("hi @bo", 6), Some((3, "bo")));
        assert_eq!(query("@", 1), Some((0, "")));
        assert_eq!(query("(@carol", 7), Some((1, "carol")));
        assert_eq!(query("mail me@home", 12), None, "not after a letter");
        assert_eq!(query("@bob is here", 12), None, "a space ends it");
        assert_eq!(
            query("hi @bo and", 6),
            Some((3, "bo")),
            "the cursor ends it"
        );
    }

    #[test]
    fn matches_names_and_later_words() {
        let candidates = [
            person("Bob", "<@2>"),
            person("Status Bot", "<@7>"),
            person("Carol", "<@3>"),
            person("bobby", "<@9>"),
        ];
        let names = |query| -> Vec<&str> {
            matching(&candidates, query, 8)
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect()
        };
        assert_eq!(names("bo"), ["Bob", "bobby", "Status Bot"]);
        assert_eq!(names(""), ["Bob", "Status Bot", "Carol", "bobby"]);
        assert!(names("x").is_empty());
        assert_eq!(matching(&candidates, "", 2).len(), 2);
    }

    #[test]
    fn mentions_go_out_in_the_network_form() {
        let candidates = [
            person("Ann", "<@1>"),
            person("Ann Lee", "<@2>"),
            person("everyone", "@everyone"),
            Candidate {
                group: true,
                ..person("Mods", "<@&20>")
            },
        ];
        assert_eq!(
            wire("@Ann Lee and @ann, ask @Mods!", &candidates),
            "<@2> and <@1>, ask <@&20>!"
        );
        assert_eq!(wire("@everyone look", &candidates), "@everyone look");
        assert_eq!(wire("@Annie stays", &candidates), "@Annie stays");
        assert_eq!(wire("me@Ann.com", &candidates), "me@Ann.com");
        assert_eq!(wire("`@Ann` but @Ann", &candidates), "`@Ann` but <@1>");
        assert_eq!(wire("café @Ann", &candidates), "café <@1>");
    }
}
