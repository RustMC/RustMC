"""Policy failure cases without network or Cargo invocation."""

import unittest

from check_dependency_licenses import review_packages


class LicensePolicyTests(unittest.TestCase):
    def test_unreviewed_and_changed_licenses_require_review(self):
        packages = [
            {"name": "rustmc-server", "version": "0.0.0", "license": "Apache-2.0"},
            {"name": "known", "version": "1.0", "license": "GPL-3.0"},
            {"name": "new", "version": "1.0", "license": None},
        ]
        problems = review_packages(packages, {"known": "MIT"})
        self.assertTrue(any("license changed" in problem for problem in problems))
        self.assertTrue(any("unreviewed dependency" in problem for problem in problems))

    def test_reviewed_metadata_passes(self):
        packages = [
            {"name": "rustmc-server", "version": "0.0.0", "license": "Apache-2.0"},
            {"name": "known", "version": "1.0", "license": "MIT"},
        ]
        self.assertEqual(review_packages(packages, {"known": "MIT"}), [])


if __name__ == "__main__":
    unittest.main()
