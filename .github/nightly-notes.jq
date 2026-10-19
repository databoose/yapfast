# The commit list in a nightly's notes, from GitHub's compare API: features
# and fixes, newest first, and the rest folded away. Merges are left out,
# since their commits are listed. GitHub names each author's account by the
# commit's email. Conventional Commit types sort them; the scripts' own
# features and fixes count as other changes.
def section($title; $lines):
  if ($lines | length) > 0 then "### \($title)\n\n" + ($lines | join("\n")) + "\n" else empty end;

[.commits | reverse | .[] | select(.parents | length == 1)
  | (.commit.message | split("\n")[0]) as $subject
  | (($subject | capture("^(?<type>[a-z]+)(\\((?<scope>[^)]*)\\))?!?: ")) // {}) as $kind
  | {
      group: (if $kind.scope == "scripts" then "other" else ($kind.type // "other") end),
      line: "- \($subject) by \(if .author then "@\(.author.login)" else .commit.author.name end) in \(.sha)"
    }]
| section("Features"; map(select(.group == "feat") | .line)),
  section("Fixes"; map(select(.group == "fix") | .line)),
  (map(select(.group != "feat" and .group != "fix") | .line)
    | if length > 0 then
        "<details><summary>Other changes (\(length))</summary>\n\n" + join("\n") + "\n\n</details>\n"
      else empty end)
