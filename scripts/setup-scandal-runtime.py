"""Install, or mirror, the ScanDAL archives Cargo.toml pins.

spiral-sdk links Spiral's ScanDAL runtime from a prebuilt static archive.
`workspace.metadata.scandal-runtime` in Cargo.toml pins spiraldb's archive for
each target: the native-runtime workflow artifact it came from and its SHA-256.
Studio keeps a copy of every pinned archive as an asset of a pre-release in its
own repository, so CI reads them with its own token.

`install [TARGET]` (the default; the host's target unless given) downloads the
archive from that release and hands it to spiraldb's setup script, fetched with
git at the pinned commit. The script verifies the archive's checksum and source
commit and installs it under SPIRAL_SCANDAL_CACHE_DIR, where spiral-scandal-sys
links it from, and prints the installed directory.

`mirror` copies every pinned archive from spiraldb's workflow artifacts into
that release; run it after re-pinning. Both use the GitHub CLI's login, or
GH_TOKEN.
"""

import argparse
import hashlib
import os
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

import tomllib

MANIFEST = Path(__file__).resolve().parent.parent / "Cargo.toml"


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "command", nargs="?", default="install", choices=["install", "mirror"]
    )
    parser.add_argument(
        "target", nargs="?", help="Rust target triple to install; defaults to the host"
    )
    arguments = parser.parse_args()
    pins = tomllib.loads(MANIFEST.read_text())["workspace"]["metadata"][
        "scandal-runtime"
    ]
    if arguments.command == "mirror":
        mirror(pins)
    else:
        install(pins, arguments.target or host_target())


def install(pins: dict, target: str) -> None:
    cache = os.environ.get("SPIRAL_SCANDAL_CACHE_DIR")
    if not cache:
        sys.exit("SPIRAL_SCANDAL_CACHE_DIR is unset; run this in the flake's shell")
    archive = pins["archives"].get(target)
    if archive is None:
        sys.exit(f"Cargo.toml pins no ScanDAL archive for {target}")
    with tempfile.TemporaryDirectory() as downloads:
        downloaded = Path(downloads) / "archive"
        subprocess.run(
            [
                *(
                    "gh",
                    "release",
                    "download",
                    pins["release"],
                    "--repo",
                    pins["mirror"],
                ),
                *("--pattern", archive_name(pins, target), "--dir", downloaded),
            ],
            check=True,
        )
        setup = setup_script(pins, Path(downloads))
        subprocess.run(
            [
                *(sys.executable, setup, "--directory", downloaded),
                *("--sha256", archive["sha256"], "--source", pins["source"]),
                *("--target", target, "--cache-dir", cache),
            ],
            check=True,
        )


def mirror(pins: dict) -> None:
    with tempfile.TemporaryDirectory() as downloads:
        archives = []
        for target, archive in pins["archives"].items():
            artifact = Path(downloads) / f"{target}.zip"
            with artifact.open("wb") as output:
                subprocess.run(
                    [
                        "gh",
                        "api",
                        f"repos/{pins['repository']}/actions/artifacts/{archive['artifact-id']}/zip",
                    ],
                    stdout=output,
                    check=True,
                )
            name = archive_name(pins, target)
            with zipfile.ZipFile(artifact) as contents:
                contents.extract(name, downloads)
            path = Path(downloads) / name
            if hashlib.sha256(path.read_bytes()).hexdigest() != archive["sha256"]:
                sys.exit(f"the {target} artifact is not the pinned archive")
            archives.append(path)
        release = ["--repo", pins["mirror"]]
        exists = subprocess.run(
            ["gh", "release", "view", pins["release"], *release],
            capture_output=True,
            check=False,
        )
        if exists.returncode != 0:
            notes = f"spiraldb's ScanDAL runtime archives for {pins['source']}, mirrored for Studio's builds."
            subprocess.run(
                [
                    *(
                        "gh",
                        "release",
                        "create",
                        pins["release"],
                        *release,
                        "--prerelease",
                    ),
                    *(
                        "--title",
                        f"ScanDAL runtime {pins['source'][:9]}",
                        "--notes",
                        notes,
                    ),
                ],
                check=True,
            )
        subprocess.run(
            [
                "gh",
                "release",
                "upload",
                pins["release"],
                *release,
                "--clobber",
                *archives,
            ],
            check=True,
        )


def archive_name(pins: dict, target: str) -> str:
    """The name spiraldb's workflow gives the archive inside its artifact."""
    return f"spiral-native-{pins['source']}-{target}.tar.gz"


def setup_script(pins: dict, directory: Path) -> Path:
    """spiraldb's setup script at the pinned commit, fetched with git."""
    checkout = directory / "spiraldb"
    git = ["git", "-C", checkout]
    checkout.mkdir()
    subprocess.run([*git, "init", "--quiet"], check=True)
    remote = f"https://github.com/{pins['repository']}.git"
    subprocess.run(
        [*git, "fetch", "--quiet", "--depth=1", remote, pins["source"]], check=True
    )
    script = directory / "setup-native-runtime.py"
    script.write_bytes(
        subprocess.check_output(
            [*git, "show", "FETCH_HEAD:scripts/setup-native-runtime.py"]
        )
    )
    return script


def host_target() -> str:
    output = subprocess.check_output(["rustc", "-vV"], text=True)
    return next(
        line.split(": ", 1)[1]
        for line in output.splitlines()
        if line.startswith("host: ")
    )


if __name__ == "__main__":
    main()
