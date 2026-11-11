import base64
import copy
import tempfile
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

import release


class ReleaseTests(unittest.TestCase):
    def test_tag_must_match_cargo_and_be_stable(self):
        current = release.version()
        release.validate_tag(f"v{current}")
        for tag in (current, "v99.0.0", "v0.1.0-beta.1", "v01.0.0", "../../other"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.validate_tag(tag)

    def test_bundle_versions_come_from_cargo(self):
        info = release.metadata("1.2.3")
        self.assertEqual(info["CFBundleVersion"], "1.2.3")
        self.assertEqual(info["CFBundleShortVersionString"], "1.2.3")
        self.assertNotIn("SUFeedURL", info)
        self.assertNotIn("SUPublicEDKey", info)

    def test_release_bundle_requires_verified_updates_and_user_install_consent(self):
        key = base64.b64encode(bytes(range(32))).decode()
        info = release.metadata("1.2.3", key)
        self.assertEqual(info["SUPublicEDKey"], key)
        self.assertEqual(info["SUFeedURL"], release.FEED)
        self.assertTrue(info["SUEnableAutomaticChecks"])
        self.assertTrue(info["SURequireSignedFeed"])
        self.assertTrue(info["SUVerifyUpdateBeforeExtraction"])
        self.assertFalse(info["SUAutomaticallyUpdate"])
        self.assertFalse(info["SUAllowsAutomaticUpdates"])
        self.assertFalse(info["SUSendProfileInfo"])
        for invalid in ("not base64", base64.b64encode(b"too short").decode()):
            with self.assertRaises(ValueError):
                release.metadata("1.2.3", invalid)

    def test_history_skips_drafts_prereleases_and_unrelated_assets(self):
        def old(tag, **extra):
            return dict(tag_name=tag, draft=False, prerelease=False,
                        assets=[{"name": release.archive_name(tag)}], **extra)

        releases = [old("v0.1.1"), old("v0.2.0"), old("v0.1.9"), old("v0.1.8")]
        releases += [dict(old("v9.0.0"), draft=True), dict(old("v8.0.0"), prerelease=True)]
        releases += [dict(old("v0.2.1"), assets=[]), dict(old("v0.1.0"), tag_name="unrelated")]
        result = release.previous_releases(releases, "v0.3.0")
        self.assertEqual([r["tag_name"] for r in result], ["v0.2.0", "v0.1.9", "v0.1.8"])
        with self.assertRaises(ValueError):
            release.previous_releases(releases, "v0.2.0")
        with self.assertRaises(ValueError):
            release.previous_releases(releases, "v0.1.0")
        self.assertEqual(release.previous_releases([], "v0.1.0"), [])

    def test_feed_requires_current_release_signed_assets_and_full_fallback(self):
        with tempfile.TemporaryDirectory() as work:
            work = Path(work)
            archive = work / release.archive_name("v1.2.3")
            archive.write_bytes(b"full archive")
            patch = work / "Whodunit1.2.3-1.2.2.delta"
            patch.write_bytes(b"delta")
            root = ET.Element("rss")
            item = ET.SubElement(ET.SubElement(root, "channel"), "item")
            ET.SubElement(item, f"{release.SPARKLE}version").text = "1.2.3"
            prefix = f"https://github.com/{release.REPO}/releases/download/v1.2.3/"

            def enclosure(parent, file, **extra):
                return ET.SubElement(parent, "enclosure", {
                    "url": prefix + file.name, "length": str(file.stat().st_size),
                    f"{release.SPARKLE}edSignature": base64.b64encode(bytes(64)).decode(), **extra,
                })

            full = enclosure(item, archive)
            enclosure(ET.SubElement(item, f"{release.SPARKLE}deltas"), patch,
                      **{f"{release.SPARKLE}deltaFrom": "1.2.2"})
            feed = work / "appcast.xml"

            def check(tree):
                ET.ElementTree(tree).write(feed)
                return release.verify_feed(feed, "v1.2.3", work)

            self.assertEqual(check(root), [archive, patch])
            for field, value in (("url", "https://example.com/update.zip"),
                                 ("url", prefix + "%2Fetc%2Fpasswd"),
                                 ("length", "1"), (f"{release.SPARKLE}edSignature", "YQ==")):
                invalid = copy.deepcopy(root)
                invalid.find("./channel/item/enclosure").set(field, value)
                with self.subTest(field=field), self.assertRaises(ValueError):
                    check(invalid)
            item.remove(full)
            with self.assertRaises(ValueError):
                check(root)
            item.append(full)
            item.find(f"{release.SPARKLE}version").text = "1.2.2"
            with self.assertRaises(ValueError):
                check(root)


if __name__ == "__main__":
    unittest.main()
