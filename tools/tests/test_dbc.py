from pathlib import Path
import struct
import sys
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TOOLS))
from dbc import read_wdbc


def make_wdbc(rows, fields, *, record_size=None, strings=b""):
    record_size = fields * 4 if record_size is None else record_size
    packed_rows = b"".join(
        struct.pack("<" + "I" * fields, *row)
        + bytes(record_size - fields * 4)
        for row in rows
    )
    return (
        b"WDBC"
        + struct.pack("<4I", len(rows), fields, record_size, len(strings))
        + packed_rows
        + strings
    )


class WdbcReaderTests(unittest.TestCase):
    def read_bytes(self, content, **options):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "test.dbc"
            path.write_bytes(content)
            return read_wdbc(path, **options)

    def test_reads_raw_records_integer_rows_and_string_block(self):
        dbc = self.read_bytes(make_wdbc([(3, 7), (4, 8)], 2, strings=b"name\0"))

        self.assertEqual(dbc.integer_rows(), [(3, 7), (4, 8)])
        self.assertEqual(dbc.integer_rows(1), [(3,), (4,)])
        self.assertEqual(dbc.records[0], struct.pack("<2I", 3, 7))
        self.assertEqual(dbc.strings, b"name\0")

    def test_strict_and_within_string_block_policies(self):
        content = make_wdbc([(1,)], 1, strings=b"x\0") + b"tail"

        with self.assertRaises(ValueError):
            self.read_bytes(content)
        self.assertEqual(
            self.read_bytes(content, string_block_policy="within").strings,
            b"x\0",
        )
        self.assertEqual(
            self.read_bytes(content, string_block_policy="ignore").strings,
            b"x\0tail",
        )

    def test_records_only_policy_keeps_legacy_talent_behavior(self):
        content = bytearray(make_wdbc([(1, 2)], 2))
        content[16:20] = struct.pack("<I", 100)

        dbc = self.read_bytes(bytes(content), minimum_fields=2, string_block_policy="ignore")
        self.assertEqual(dbc.integer_rows(), [(1, 2)])

    def test_rejects_invalid_header_truncated_record_and_field_mismatch(self):
        with self.assertRaises(ValueError):
            self.read_bytes(b"not a dbc")
        with self.assertRaises(ValueError):
            self.read_bytes(make_wdbc([(1, 2)], 2)[:-1])
        with self.assertRaises(ValueError):
            self.read_bytes(make_wdbc([(1,)], 1), minimum_fields=2)
        with self.assertRaises(ValueError):
            self.read_bytes(
                make_wdbc([(1,)], 1, record_size=8),
                require_exact_record_size=True,
            )

    def test_custom_record_minimum_and_field_extent_policy(self):
        content = make_wdbc([(1, 2, 3)], 3, record_size=44)

        dbc = self.read_bytes(
            content,
            minimum_fields=3,
            minimum_record_bytes=44,
            validate_field_extent=False,
        )
        self.assertEqual(dbc.integer_rows(), [(1, 2, 3)])


if __name__ == "__main__":
    unittest.main()
