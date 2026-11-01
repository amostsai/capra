#!/usr/bin/env python3
"""Package an existing release binary for the current Ubuntu amd64 host."""
import argparse
import hashlib
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import tomllib


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    system = platform.freedesktop_os_release()
    if system.get("ID") != "ubuntu" or platform.machine() != "x86_64":
        parser.error("This packager requires an Ubuntu x86_64 host.")
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    release = system["VERSION_ID"]
    name = f"capra-{version}-ubuntu{release}-x86_64"
    output = root / "dist"
    output.mkdir(exist_ok=True)
    executable = output / "capra"
    if binary != executable:
        shutil.copy2(binary, executable)
    executable.chmod(0o755)
    with tempfile.TemporaryDirectory(prefix="capra-package-") as temp:
        stage = Path(temp)
        # Let the host package database calculate actual native library floors.
        debian = stage / "debian"
        debian.mkdir()
        (debian / "control").write_text("Source: capra\n\nPackage: capra\nArchitecture: amd64\n")
        dependencies = subprocess.check_output(
            ["dpkg-shlibdeps", "-O", f"-e{binary}"], cwd=stage, text=True
        ).strip().removeprefix("shlibs:Depends=")
        runtime = [
            "ffmpeg", "gstreamer1.0-x", "gstreamer1.0-pulseaudio",
            "gstreamer1.0-pipewire", "gstreamer1.0-plugins-base",
            "gstreamer1.0-plugins-good", "gstreamer1.0-plugins-bad",
            "gstreamer1.0-plugins-ugly", "gstreamer1.0-libav",
            "pulseaudio-utils", "libvulkan1", "xdg-utils", "xdg-desktop-portal",
        ]
        dependencies += ", " + ", ".join(runtime)
        instructions = (
            f"capra {version} — Ubuntu {release} x86_64\n\n"
            f"Built on Ubuntu {release}; this binary is not a build for older Ubuntu releases.\n"
            "Install the .deb with: sudo apt install ./capra_*.deb\n"
            "The package manager installs runtime dependencies. A working Vulkan driver is required.\n"
            "Wayland recording requires the desktop's xdg-desktop-portal backend.\n\n"
            "For this tar archive, install dependencies first:\n"
            "sudo apt install " + " ".join(runtime) + " fonts-noto-cjk\n"
            "Then run ./capra from the extracted directory.\n\n"
            "Settings: ${XDG_CONFIG_HOME:-$HOME/.config}/capra/settings.json\n"
            "Recordings default to $HOME/Videos.\n"
            "Recordings use MKV, then convert to MP4 safely after stopping.\n"
            "This package contains no recordings or personal settings.\n\n"
            f"Native library requirements: {dependencies}\n"
        )
        portable = stage / name
        portable.mkdir()
        shutil.copy2(binary, portable / "capra")
        shutil.copy2(root / "assets/icons/512x512/apps/capra.png", portable / "capra.png")
        (portable / "README.txt").write_text(instructions)
        with tarfile.open(output / f"{name}.tar.gz", "w:gz") as archive:
            archive.add(portable, arcname=name)
        package = stage / "package"
        (package / "DEBIAN").mkdir(parents=True)
        (package / "usr/bin").mkdir(parents=True)
        shutil.copy2(binary, package / "usr/bin/capra")
        applications = package / "usr/share/applications"
        applications.mkdir(parents=True)
        shutil.copytree(root / "assets/icons", package / "usr/share/icons/hicolor")
        (applications / "capra.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=capra\nName[zh_TW]=capra\n"
            "Comment=Screen and audio recorder\nComment[zh_TW]=螢幕畫面與聲音錄影工具\n"
            "Exec=capra\nIcon=capra\nTerminal=false\n"
            "Categories=AudioVideo;Recorder;Utility;\nStartupNotify=true\nStartupWMClass=capra\n"
        )
        documentation = package / "usr/share/doc/capra"
        documentation.mkdir(parents=True)
        (documentation / "README.txt").write_text(instructions)
        (package / "DEBIAN/control").write_text(
            f"Package: capra\nVersion: {version}\nArchitecture: amd64\n"
            "Section: video\nPriority: optional\nMaintainer: capra contributors\n"
            f"Installed-Size: {(binary.stat().st_size + 1023) // 1024}\n"
            f"Depends: {dependencies}\nRecommends: fonts-noto-cjk\n"
            f"Description: Screen and audio recorder for Ubuntu {release}\n"
            " GPUI desktop recorder with crash-resilient MKV capture and MP4 conversion.\n"
        )
        deb = output / f"capra_{version}_ubuntu{release}_amd64.deb"
        subprocess.run(["dpkg-deb", "--root-owner-group", "--build", str(package), str(deb)], check=True)
        files = [executable, deb, output / f"{name}.tar.gz"]
        (output / "SHA256SUMS").write_text("".join(
            f"{hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()}  {path.name}\n"
            for path in files
        ))
        for path in files:
            print(path)


if __name__ == "__main__":
    main()
