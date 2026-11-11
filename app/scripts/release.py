#!/usr/bin/env python3
"""Package the Mac app and publish Sparkle artifacts alongside GitHub releases."""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tempfile
from urllib.parse import unquote, urlparse
import xml.etree.ElementTree as ET

APP = Path(__file__).resolve().parents[1]
REPO = "vaporwavie/whodunit"
FEED = f"https://github.com/{REPO}/releases/latest/download/appcast.xml"
SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"


def run(*args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def version_tuple(tag):
    match = re.fullmatch(r"v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag)
    if not match:
        raise ValueError(f"Expected a stable version such as v0.2.0, got {tag!r}")
    return tuple(map(int, match.groups()))


def version():
    response = run("cargo", "metadata", "--offline", "--no-deps", "--format-version", "1",
                   "--manifest-path", APP / "Cargo.toml", capture_output=True, text=True, cwd=APP)
    return next(package["version"] for package in json.loads(response.stdout)["packages"]
                if package["name"] == "whodunit")


def validate_tag(tag):
    version_tuple(tag)
    if tag != f"v{version()}":
        raise ValueError(f"Tag {tag!r} must match app/Cargo.toml (v{version()})")


def archive_name(tag):
    return f"Whodunit-{tag.removeprefix('v')}-macos-universal.zip"


def metadata(app_version, public_key=None):
    version_tuple(app_version)
    info = {
        "CFBundleName": "Whodunit",
        "CFBundleIdentifier": "io.github.vaporwavie.whodunit",
        "CFBundleExecutable": "whodunit",
        "CFBundleIconFile": "AppIcon",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": app_version,
        "CFBundleVersion": app_version,
        "LSMinimumSystemVersion": "12.0",
        "NSHighResolutionCapable": True,
    }
    if public_key:
        if len(base64.b64decode(public_key, validate=True)) != 32:
            raise ValueError("SPARKLE_PUBLIC_KEY must be a base64-encoded Ed25519 public key")
        info.update({
            "SUFeedURL": FEED,
            "SUPublicEDKey": public_key,
            "SUVerifyUpdateBeforeExtraction": True,
            "SURequireSignedFeed": True,
            # Checks are automatic; installation/relaunch still require consent.
            "SUEnableAutomaticChecks": True,
            "SUAutomaticallyUpdate": False,
            "SUAllowsAutomaticUpdates": False,
            "SUSendProfileInfo": False,
        })
    return info


def package(binary, output, sparkle=None):
    public_key = os.environ.get("SPARKLE_PUBLIC_KEY")
    if bool(sparkle) != bool(public_key):
        raise ValueError("--sparkle and SPARKLE_PUBLIC_KEY must be provided together")
    output = output.resolve()
    if output.suffix != ".app" or output.exists():
        raise ValueError("Output must be a new .app directory")
    contents = output / "Contents"
    (contents / "MacOS").mkdir(parents=True)
    resources = contents / "Resources"
    resources.mkdir()
    shutil.copy2(APP.parent / "LICENSE", resources / "Whodunit-LICENSE")
    shutil.copy2(binary, contents / "MacOS/whodunit")
    (contents / "MacOS/whodunit").chmod(0o755)
    with (contents / "Info.plist").open("wb") as destination:
        plistlib.dump(metadata(version(), public_key), destination)
    with tempfile.TemporaryDirectory() as work:
        iconset = Path(work) / "AppIcon.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                suffix = "@2x" if scale == 2 else ""
                run("sips", "-z", size * scale, size * scale, APP / "assets/icons/app.png",
                    "--out", iconset / f"icon_{size}x{size}{suffix}.png", stdout=subprocess.DEVNULL)
        run("iconutil", "-c", "icns", iconset, "-o", resources / "AppIcon.icns")
    identity = os.environ.get("APPLE_SIGNING_IDENTITY", "-")

    def sign(path):
        options = ["--options", "runtime", "--timestamp"] if identity != "-" else []
        if path.name == "Downloader.xpc":
            options.append("--preserve-metadata=entitlements")
        run("codesign", "--force", "--sign", identity, *options, path)

    if sparkle:
        framework = contents / "Frameworks/Sparkle.framework"
        framework.parent.mkdir()
        run("ditto", sparkle / "Sparkle.framework", framework)
        shutil.copy2(sparkle / "LICENSE", resources / "Sparkle-LICENSE")
        # Sign inside-out, without --deep, so each helper retains its own identifier.
        inner = framework / "Versions/B"
        for path in (inner / "XPCServices/Downloader.xpc", inner / "XPCServices/Installer.xpc",
                     inner / "Autoupdate", inner / "Updater.app", framework):
            sign(path)
    sign(output)
    run("codesign", "--verify", "--deep", "--strict", output)


def previous_releases(releases, tag, limit=3):
    current = version_tuple(tag)
    candidates = []
    for release in releases:
        if release["draft"] or release["prerelease"]:
            continue
        try:
            previous = version_tuple(release["tag_name"])
        except ValueError:
            continue
        if previous >= current:
            raise ValueError(f"Release {tag} must be newer than {release['tag_name']}")
        if archive_name(release["tag_name"]) in {asset["name"] for asset in release["assets"]}:
            candidates.append(release)
    return sorted(candidates, key=lambda r: version_tuple(r["tag_name"]), reverse=True)[:limit]


def history(tag, output):
    validate_tag(tag)
    response = run("gh", "api", "--paginate", "--slurp", f"repos/{REPO}/releases",
                   capture_output=True, text=True)
    releases = [release for page in json.loads(response.stdout) for release in page]
    output.mkdir(parents=True, exist_ok=True)
    for release in previous_releases(releases, tag):
        run("gh", "release", "download", release["tag_name"], "--repo", REPO,
            "--pattern", archive_name(release["tag_name"]), "--dir", output)


def verify_feed(feed, tag, archives):
    """Reject feeds that reference missing assets or an accidental development build."""
    root = ET.parse(feed).getroot()
    items = root.findall("./channel/item")
    if len(items) != 1 or items[0].findtext(f"{SPARKLE}version") != tag[1:]:
        raise ValueError("Feed must contain exactly the release being published")
    enclosures = items[0].findall(".//enclosure")
    if not enclosures or items[0].find("enclosure") is None:
        raise ValueError("Feed must include a full archive fallback")
    prefix = f"https://github.com/{REPO}/releases/download/{tag}/"
    assets = []
    for enclosure in enclosures:
        url = enclosure.attrib["url"]
        if not url.startswith(prefix):
            raise ValueError(f"Update URL is outside this release: {url}")
        name = unquote(urlparse(url).path.rsplit("/", 1)[1])
        if not name or Path(name).name != name:
            raise ValueError(f"Invalid asset name: {name}")
        asset = archives / name
        signature = base64.b64decode(enclosure.attrib[f"{SPARKLE}edSignature"], validate=True)
        if len(signature) != 64 or int(enclosure.attrib["length"]) != asset.stat().st_size:
            raise ValueError(f"Invalid signature or length for {name}")
        assets.append(asset)
    if archive_name(tag) not in {asset.name for asset in assets}:
        raise ValueError("Full archive has an unexpected filename")
    return assets


def appcast(sparkle, tag, archives, output):
    validate_tag(tag)
    key = os.environ["SPARKLE_PRIVATE_KEY"]
    # Verify the private seed matches the public key embedded in the app before publishing.
    run("swift", APP / "scripts/verify-update-key.swift")
    run(sparkle / "bin/generate_appcast", "--ed-key-file", "-", "--versions", tag[1:],
        "--maximum-versions", "1", "--maximum-deltas", "3", "--download-url-prefix",
        f"https://github.com/{REPO}/releases/download/{tag}/", archives, input=key, text=True)
    feed = archives / "appcast.xml"
    assets = verify_feed(feed, tag, archives)
    run(sparkle / "bin/sign_update", "--verify", "--ed-key-file", "-", feed, input=key, text=True)
    for enclosure in ET.parse(feed).findall("./channel/item/.//enclosure"):
        signature = enclosure.attrib[f"{SPARKLE}edSignature"]
        asset = archives / unquote(urlparse(enclosure.attrib["url"]).path.rsplit("/", 1)[1])
        run(sparkle / "bin/sign_update", "--verify", "--ed-key-file", "-", asset, signature,
            input=key, text=True)
        previous = enclosure.attrib.get(f"{SPARKLE}deltaFrom")
        if previous:
            with tempfile.TemporaryDirectory() as work:
                source = Path(work) / "source"
                patched = Path(work) / "Whodunit.app"
                run("ditto", "-x", "-k", archives / archive_name(previous), source)
                run(sparkle / "bin/BinaryDelta", "apply", source / "Whodunit.app", patched, asset)
                run("codesign", "--verify", "--deep", "--strict", patched)
    output.mkdir(parents=True, exist_ok=True)
    for asset in [feed, *assets]:
        shutil.copy2(asset, output / asset.name)
    with (output / "SHA256SUMS").open("w") as checksums:
        for asset in sorted(output.iterdir()):
            if asset.name != "SHA256SUMS":
                checksums.write(f"{hashlib.sha256(asset.read_bytes()).hexdigest()}  {asset.name}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    validate = commands.add_parser("validate")
    validate.add_argument("tag")
    bundle = commands.add_parser("package")
    bundle.add_argument("--binary", type=Path, required=True)
    bundle.add_argument("--output", type=Path, required=True)
    bundle.add_argument("--sparkle", type=Path)
    old = commands.add_parser("history")
    old.add_argument("--tag", required=True)
    old.add_argument("--output", type=Path, required=True)
    feed = commands.add_parser("appcast")
    feed.add_argument("--sparkle", type=Path, required=True)
    feed.add_argument("--tag", required=True)
    feed.add_argument("--archives", type=Path, required=True)
    feed.add_argument("--output", type=Path, required=True)
    args = vars(parser.parse_args())
    command = args.pop("command")
    {"validate": validate_tag, "package": package, "history": history, "appcast": appcast}[command](**args)


if __name__ == "__main__":
    main()
