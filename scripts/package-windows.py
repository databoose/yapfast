#!/usr/bin/env python3
"""Package the Windows executable, build inputs and dependency notices."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import tarfile
import urllib.parse
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-pc-windows-msvc"
SOURCE_FILES = ("Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain.toml", "README.md", ".gitignore")
SOURCE_DIRECTORIES = (".cargo", "src", "ui", "packaging", "crates", "scripts", "nix", "vendor", "testing", "docs")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def zip_directory(source, destination):
    # Nix inputs use an epoch timestamp, which predates ZIP's 1980 minimum.
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6, strict_timestamps=False) as archive:
        for path in sorted(source.rglob("*")):
            if path.is_file():
                archive.write(path, source.name + "/" + path.relative_to(source).as_posix())


def license_file(path):
    name = path.name.upper()
    return path.is_file() and name.startswith(("LICENSE", "COPYING", "NOTICE", "OFL"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, default=ROOT / "target" / TARGET / "release/yapfast.exe")
    parser.add_argument("--output", type=Path, default=ROOT / "target/dist")
    args = parser.parse_args()
    executable = args.exe.resolve()
    newest_input = max(path.stat().st_mtime for path in
                       [ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "build.rs",
                        *list((ROOT / "src").rglob("*.rs")), *list((ROOT / "ui").rglob("*")),
                        *list((ROOT / "packaging").rglob("*"))]
                       if path.is_file())
    if executable.stat().st_mtime < newest_input:
        raise SystemExit("Windows executable predates the current UI; run scripts/build-windows first")
    with executable.open("rb") as stream:
        header = stream.read(64)
        if header[:2] != b"MZ":
            raise SystemExit("Expected a Windows executable")
        stream.seek(struct.unpack_from("<I", header, 60)[0])
        pe = stream.read(6)
        if pe[:4] != b"PE\0\0" or struct.unpack_from("<H", pe, 4)[0] != 0x8664:
            raise SystemExit("Expected an AMD64 Windows PE executable")
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", TARGET],
        cwd=ROOT, timeout=30,
    ))
    version = next(package["version"] for package in metadata["packages"] if package["name"] == "yapfast")
    args.output.mkdir(parents=True, exist_ok=True)
    archive_path = args.output / f"Yapfast-{version}-windows-x64.zip"
    source_path = args.output / f"Yapfast-{version}-source.zip"
    with tempfile.TemporaryDirectory(prefix="yapfast-windows-package-") as temporary:
        temporary = Path(temporary)
        source = temporary / f"Yapfast-{version}-source"
        source.mkdir()
        # Only what the repository tracks: the folders also hold private,
        # ignored material.
        tracked = subprocess.check_output(["git", "ls-files", "-z", "--", *SOURCE_FILES, *SOURCE_DIRECTORIES],
                                          cwd=ROOT, timeout=30).decode().split("\0")
        for name in filter(None, tracked):
            (source / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, source / name)
        # Include the exact upstream RTC source used by the AGPL dependencies.
        rtc = next(package for package in metadata["packages"] if package["name"] == "matrix-rtc-core")
        rtc_root = Path(rtc["manifest_path"]).parent.parent.parent
        shutil.copytree(rtc_root, source / "upstream/matrix-rust-rtc", ignore=shutil.ignore_patterns(".git", "target", "node_modules"))
        zip_directory(source, source_path)

        package = temporary / f"Yapfast-{version}-windows-x64"
        package.mkdir()
        shutil.copy2(executable, package / "yapfast.exe")
        shutil.copy2(source_path, package / source_path.name)
        (package / "README.txt").write_text(
            "Yapfast private beta for 64-bit Windows 10/11\n\n"
            "1. Extract the entire ZIP to a folder.\n"
            "2. Double-click yapfast.exe.\n"
            "3. Keep the homeserver https://matrix.org.\n"
            "4. Enter bob and the password supplied privately by the account owner.\n"
            "5. Open Yapfast beta, then general or Lounge. Voice starts muted.\n\n"
            "Passwords, recovery keys and saved sessions are not included.\n"
            "No Rust, Visual Studio, or separate app installation is required.\n"
            "Yapfast writes beta diagnostics to talk.log in its data folder,\n"
            "%LOCALAPPDATA%\\Yapfast\\Yapfast\\data, or to the console run-yapfast.cmd opens.\n"
            "Windows stores session secrets in Credential Manager and app data\n"
            "under your Windows user profile. Keep the first verified session.\n\n"
            "Voice: select Lounge, Join voice, choose devices, then Unmute.\n"
            "Use headphones for the first two-machine audio test.\n"
            "If anything fails, record the error text, Windows version, GPU and\n"
            "audio device model. Never send your password or recovery key in logs.\n\n"
            "If Yapfast closes immediately, run run-yapfast.cmd to keep its error visible.\n\n"
            "Yapfast switches to software rendering if the GPU cannot start.\n"
            "To choose it yourself, run run-yapfast.cmd --renderer software.\n"
            "To retry the GPU after a fallback, run run-yapfast.cmd --renderer gpu.\n\n"
            "Source snapshot, dependency notices and original licenses are included.\n",
            encoding="utf-8",
        )
        (package / "run-yapfast.cmd").write_text(
            '@echo off\ncd /d "%~dp0"\nyapfast.exe %*\n'
            'if errorlevel 1 (\n  echo Yapfast exited with an error.\n  pause\n)\n',
            encoding="utf-8", newline="\r\n",
        )
        dependencies = []
        licenses = package / "licenses"
        licenses.mkdir()
        for item in sorted(metadata["packages"], key=lambda value: (value["name"], value["version"])):
            if item["name"] == "yapfast":
                continue
            directory = Path(item["manifest_path"]).parent
            target = licenses / (item["name"] + "-" + item["version"])
            files = set()
            for parent in (directory, directory.parent, directory.parent.parent):
                files.update(path for path in parent.iterdir() if license_file(path))
            files.update(path for path in directory.rglob("*") if license_file(path))
            copied = []
            for index, path in enumerate(sorted(files)):
                target.mkdir(exist_ok=True)
                name = f"{index}-{path.name}"
                shutil.copy2(path, target / name)
                copied.append((target / name).relative_to(package).as_posix())
            dependencies.append({
                "name": item["name"], "version": item["version"], "license": item["license"],
                "source": item["source"], "repository": item["repository"], "license_files": copied,
            })
        skia_location = os.environ.get("SKIA_BINARIES_URL")
        if not skia_location:
            archives = list((ROOT / ".local/windows/skia-static-production/prepared").glob("*-static.tar.gz"))
            if len(archives) != 1:
                raise SystemExit("Prepare the pinned Windows Skia archive before packaging")
            skia_location = archives[0].as_uri()
        skia_url = urllib.parse.urlparse(skia_location)
        if skia_url.scheme != "file":
            raise SystemExit("Packaging requires the pinned local Skia archive")
        skia_archive = Path(urllib.parse.unquote(skia_url.path))
        with tarfile.open(skia_archive) as native:
            for member in native.getmembers():
                if member.isfile() and Path(member.name).name.upper().startswith(("LICENSE", "COPYING", "NOTICE")):
                    with native.extractfile(member) as notice:
                        (licenses / ("skia-native-" + Path(member.name).name)).write_bytes(notice.read())
        skia_source = Path(os.environ["YAPFAST_SKIA_WINDOWS_SOURCE"])
        for notice in skia_source.rglob("*"):
            if license_file(notice):
                destination = licenses / "skia-source" / notice.relative_to(skia_source)
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(notice, destination)
        webrtc = Path(os.environ["LK_CUSTOM_WEBRTC"])
        shutil.copy2(webrtc / "LICENSE.md", licenses / "libwebrtc-LICENSE.md")
        (package / "THIRD-PARTY.json").write_text(json.dumps(dependencies, indent=2) + "\n", encoding="utf-8")
        build = {
            "version": version, "target": TARGET,
            "rustc": subprocess.check_output(["rustc", "--version"], text=True, timeout=10).strip(),
            "cargo_xwin": subprocess.check_output(["cargo", "xwin", "--version"], text=True, timeout=10).strip(),
            "ui_framework": "Slint 1.18.1",
            "renderer_default": "skia-opengl, falling back to skia-software",
            "renderer_software": "skia-software",
            "skia_version": "0.153.3",
            "skia_source_revision": "61e7ca4e99062cdd0ab69445d5963fb3365778f6",
            "skia_source_archive_sha256": "8401d0de878582a03abc1029bb62330aea58cda89904ae6ab187302e6d48ba5d",
            "skia_runtime": "MT_StaticRelease",
            "skia_archive_prepared_sha256": digest(skia_archive),
            "homeserver_default": "https://matrix.org",
            "cargo_lock_sha256": digest(ROOT / "Cargo.lock"),
            "executable_sha256": digest(executable),
            "source_archive_sha256": digest(source_path),
            "webrtc_tag": "webrtc-51ef663",
            "webrtc_archive_sha256": "0a56a5c91b3b7b4222082b8a09388d16a802cde22d67295beb24c55700755b30",
        }
        (package / "BUILD.json").write_text(json.dumps(build, indent=2) + "\n", encoding="utf-8")
        zip_directory(package, archive_path)
    hashes = args.output / "SHA256SUMS.txt"
    hashes.write_text(f"{digest(archive_path)}  {archive_path.name}\n{digest(source_path)}  {source_path.name}\n", encoding="utf-8")
    print(f"Packaged {archive_path} ({archive_path.stat().st_size / 1024 / 1024:.1f} MiB)")
    print(f"Source snapshot: {source_path}")
    print(f"Checksums: {hashes}")


if __name__ == "__main__":
    main()
