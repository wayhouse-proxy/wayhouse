#!/usr/bin/env python3
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import check_plugins as cp  # noqa: E402


def module(major, minor):
    return b"\0asm\1\0\0\0" + cp.SECTION + major.to_bytes(2, "little") + minor.to_bytes(2, "little") + b"rest"


class CheckPlugins(unittest.TestCase):
    def lock(self, text):
        with tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d) / "plugins.lock"
            p.write_text(text)
            return cp.load_lock(p)

    def test_the_repo_lock_parses(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        repo, rev = cp.load_lock(root / "plugins.lock")
        self.assertEqual(repo, "wayhouse-proxy/plugins")
        self.assertEqual(len(rev), 40)

    def test_a_lock_needs_a_full_sha_and_a_plain_repo(self):
        for bad in ('repo = "a/b"\nrev = "main"\n', 'repo = "a/b"\nrev = "' + "A" * 40 + '"\n', 'repo = "../x"\nrev = "' + "a" * 40 + '"\n'):
            with self.assertRaises(ValueError):
                self.lock(bad)

    def test_the_abi_minor_is_read_from_the_module_and_from_the_crate(self):
        self.assertEqual(cp.wasm_abi_minor(module(0, 3)), 3)
        with self.assertRaises(ValueError):
            cp.wasm_abi_minor(b"\0asm")
        with self.assertRaises(ValueError):
            cp.wasm_abi_minor(module(1, 0))
        self.assertEqual(cp.host_abi_minor("pub const ABI_MINOR: u16 = 7;"), 7)
        root = pathlib.Path(__file__).resolve().parents[2]
        self.assertIsInstance(cp.host_abi_minor((root / "crates/wayhouse-plugin-abi/src/lib.rs").read_text()), int)


if __name__ == "__main__":
    unittest.main()
