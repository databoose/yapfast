Atkinson Hyperlegible Next by the Braille Institute, the UI's typeface: the
static TTF styles Regular, Italic, Medium, Bold, Bold Italic and ExtraBold,
copied unmodified from the googlefonts/atkinson-hyperlegible-next repository at
commit 7925f50f649b3813257faf2f4c0b381011f434f1 (version 2.001, through
nixpkgs' `atkinson-hyperlegible-next`) on 2026-10-07. They're embedded in the
Talk binary by the Slint compiler. `LICENSE-OFL.txt` is their SIL Open Font
License 1.1, which permits embedding, modifying and redistributing them,
including in a public repository.

Satoshi by Indian Type Foundry, the second choice, kept here but not embedded:
its imports are commented out at the top of `ui/talk.slint`. Downloaded
unmodified from Fontshare (https://www.fontshare.com/fonts/satoshi) on
2026-10-03, in the static TTF styles Regular, Italic, Medium, Bold, Bold Italic
and Black.

`LICENSE-FFL.txt` is Satoshi's ITF Free Font License 2.0. It permits embedding
the fonts in desktop applications. It forbids modifying, subsetting or
converting the files, and forbids redistributing them on their own, including
through a publicly accessible repository. Talk's repository is private; remove
the Satoshi files, or replace them with a download step, before publishing the
source.
