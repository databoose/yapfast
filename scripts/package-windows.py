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
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-pc-windows-msvc"
SOURCE_FILES = ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "README.md", "PRODUCT.md", "DESIGN.md", ".gitignore")
SOURCE_DIRECTORIES = (".cargo", "src", "crates", "scripts", "nix", "vendor", "testing", "deploy", "docs")


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
    parser.add_argument("--exe", type=Path, default=ROOT / "target" / TARGET / "release/talk.exe")
    parser.add_argument("--output", type=Path, default=ROOT / "target/dist")
    args = parser.parse_args()
    executable = args.exe.resolve()
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
    version = next(package["version"] for package in metadata["packages"] if package["name"] == "talk")
    args.output.mkdir(parents=True, exist_ok=True)
    archive_path = args.output / f"Talk-{version}-windows-x64.zip"
    source_path = args.output / f"Talk-{version}-source.zip"
    with tempfile.TemporaryDirectory(prefix="talk-windows-package-") as temporary:
        temporary = Path(temporary)
        source = temporary / f"Talk-{version}-source"
        source.mkdir()
        for name in SOURCE_FILES:
            shutil.copy2(ROOT / name, source / name)
        for name in SOURCE_DIRECTORIES:
            shutil.copytree(ROOT / name, source / name, ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".git", "review"))
        # Include the exact upstream RTC source used by the AGPL dependencies.
        rtc = next(package for package in metadata["packages"] if package["name"] == "matrix-rtc-core")
        rtc_root = Path(rtc["manifest_path"]).parent.parent.parent
        shutil.copytree(rtc_root, source / "upstream/matrix-rust-rtc", ignore=shutil.ignore_patterns(".git", "target", "node_modules"))
        zip_directory(source, source_path)

        package = temporary / f"Talk-{version}-windows-x64"
        package.mkdir()
        shutil.copy2(executable, package / "talk.exe")
        shutil.copy2(source_path, package / source_path.name)
        (package / "README.txt").write_text(
            "Talk private beta for 64-bit Windows 10/11\n\n"
            "1. Extract the entire ZIP to a folder.\n"
            "2. Double-click talk.exe.\n"
            "3. Keep the homeserver https://matrix.org.\n"
            "4. Enter bob and the password supplied privately by the account owner.\n"
            "5. Open Talk beta, then general or Lounge. Voice starts muted.\n\n"
            "Passwords, recovery keys and saved sessions are not included.\n"
            "No Rust, Visual Studio, or separate app installation is required.\n"
            "A console window shows beta diagnostics and closes when Talk quits.\n"
            "Windows stores session secrets in Credential Manager and app data\n"
            "under your Windows user profile. Keep the first verified session.\n\n"
            "Voice: select Lounge, Join voice, choose devices, then Unmute.\n"
            "Use headphones for the first two-machine audio test.\n"
            "If anything fails, record the error text, Windows version, GPU and\n"
            "audio device model. Never send your password or recovery key in logs.\n\n"
            "If Talk closes immediately, run run-talk.cmd to keep its error visible.\n\n"
            "Source snapshot, dependency notices and original licenses are included.\n",
            encoding="utf-8",
        )
        (package / "run-talk.cmd").write_text(
            '@echo off\ncd /d "%~dp0"\ntalk.exe %*\n'
            'if errorlevel 1 (\n  echo Talk exited with an error.\n  pause\n)\n',
            encoding="utf-8", newline="\r\n",
        )
        dependencies = []
        licenses = package / "licenses"
        licenses.mkdir()
        for item in sorted(metadata["packages"], key=lambda value: (value["name"], value["version"])):
            if item["name"] == "talk":
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
        webrtc = Path(os.environ["LK_CUSTOM_WEBRTC"])
        shutil.copy2(webrtc / "LICENSE.md", licenses / "libwebrtc-LICENSE.md")
        (package / "THIRD-PARTY.json").write_text(json.dumps(dependencies, indent=2) + "\n", encoding="utf-8")
        build = {
            "version": version, "target": TARGET,
            "rustc": subprocess.check_output(["rustc", "--version"], text=True, timeout=10).strip(),
            "cargo_xwin": subprocess.check_output(["cargo", "xwin", "--version"], text=True, timeout=10).strip(),
            "shader_compiler_package": "Microsoft.Windows.SDK.CPP 10.0.26100.8249",
            "shader_compiler_package_sha256": "f8787b2f6678164ae789bdca6247e696c2a0f529a39ceb969d6ef3d69a987131",
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
