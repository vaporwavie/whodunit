"""Offline integration test; set SPARKLE_TEST_TOOLS to a fetched Sparkle distribution."""

import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import release


@unittest.skipUnless(sys.platform == "darwin" and os.environ.get("SPARKLE_TEST_TOOLS"),
                     "requires macOS and SPARKLE_TEST_TOOLS (no network or Apple credentials)")
class SparkleTests(unittest.TestCase):
    def test_signed_feed_delta_and_full_fallback(self):
        sparkle = Path(os.environ["SPARKLE_TEST_TOOLS"])
        keys = release.run("swift", "-e", """
import Foundation
import CryptoKit
let key = Curve25519.Signing.PrivateKey()
print(String(data: try! JSONSerialization.data(withJSONObject: [
    "SPARKLE_PRIVATE_KEY": key.rawRepresentation.base64EncodedString(),
    "SPARKLE_PUBLIC_KEY": key.publicKey.rawRepresentation.base64EncodedString()
]), encoding: .utf8)!)
""", capture_output=True, text=True)
        environment = json.loads(keys.stdout)
        environment["APPLE_SIGNING_IDENTITY"] = "-"
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, environment):
            work = Path(directory)
            source = work / "main.c"
            source.write_text("int main(void) { return 0; }\n")
            binary = work / "whodunit"
            release.run("clang", source, "-o", binary)
            bundle = work / "Whodunit.app"
            release.package(binary, bundle, sparkle)
            resource = bundle / "Contents/Resources/fixture.bin"
            resource.write_bytes(os.urandom(1024 * 1024))
            release.run("codesign", "--force", "--sign", "-", bundle)
            old = work / "old/Whodunit.app"
            release.run("ditto", bundle, old)
            with (old / "Contents/Info.plist").open("rb") as file:
                info = plistlib.load(file)
            info["CFBundleVersion"] = info["CFBundleShortVersionString"] = "0.0.0"
            with (old / "Contents/Info.plist").open("wb") as file:
                plistlib.dump(info, file)
            with (old / "Contents/Resources/fixture.bin").open("r+b") as file:
                file.write(b"previous release")
            release.run("codesign", "--force", "--sign", "-", old)
            archives = work / "archives"
            archives.mkdir()
            tag = f"v{release.version()}"
            for app, name in ((bundle, release.archive_name(tag)),
                              (old, release.archive_name("v0.0.0"))):
                release.run("ditto", "-c", "-k", "--keepParent", app, archives / name)
            output = work / "publish"
            release.appcast(sparkle, tag, archives, output)
            deltas = list(output.glob("*.delta"))
            self.assertEqual(len(deltas), 1)
            self.assertLess(deltas[0].stat().st_size, (output / release.archive_name(tag)).stat().st_size)
            self.assertTrue((output / "appcast.xml").is_file())
            self.assertTrue((output / "SHA256SUMS").is_file())
            self.assertFalse((output / release.archive_name("v0.0.0")).exists())
            # A first release still publishes a valid signed feed, with no delta.
            shutil.rmtree(archives)
            archives.mkdir()
            shutil.copy2(output / release.archive_name(tag), archives)
            first = work / "first"
            release.appcast(sparkle, tag, archives, first)
            self.assertEqual(list(first.glob("*.delta")), [])
            # No release may be signed by a key other than the one embedded in the app.
            os.environ["SPARKLE_PUBLIC_KEY"] = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
            with self.assertRaises(subprocess.CalledProcessError):
                release.appcast(sparkle, tag, archives, work / "wrong-key")


if __name__ == "__main__":
    unittest.main()
