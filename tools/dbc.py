"""Shared low-level reader for WotLK WDBC files."""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import struct


@dataclass(frozen=True)
class WdbcFile:
    field_count: int
    record_size: int
    records: tuple[bytes, ...]
    strings: bytes
    source: bytes

    def integer_rows(self, field_count: int | None = None) -> list[tuple[int, ...]]:
        fields = self.field_count if field_count is None else field_count
        if fields < 0 or fields > self.field_count or fields * 4 > self.record_size:
            raise ValueError("requested WDBC fields exceed the record layout")
        return [
            struct.unpack_from("<" + "I" * fields, record)
            for record in self.records
        ]


def read_wdbc(
    path: Path,
    *,
    minimum_fields: int = 0,
    minimum_record_bytes: int | None = None,
    require_exact_record_size: bool = False,
    validate_field_extent: bool = True,
    string_block_policy: str = "exact",
    error_type: type[Exception] = ValueError,
) -> WdbcFile:
    data = path.read_bytes()
    if len(data) < 20 or data[:4] != b"WDBC":
        raise error_type(f"{path} has an invalid WDBC header")
    count, fields, record_size, string_size = struct.unpack_from("<4I", data, 4)
    invalid_record_size = validate_field_extent and (
        record_size != fields * 4 if require_exact_record_size else record_size < fields * 4
    )
    required_bytes = fields * 4 if minimum_record_bytes is None else minimum_record_bytes
    if fields < minimum_fields or invalid_record_size or record_size < required_bytes:
        raise error_type(f"{path} has an unsupported record layout")
    records_end = 20 + count * record_size
    file_end = records_end + string_size
    if records_end > len(data):
        raise ValueError(f"{path} has truncated records")
    if string_block_policy not in {"exact", "within", "ignore"}:
        raise ValueError(f"unsupported WDBC string block policy: {string_block_policy}")
    if string_block_policy == "exact" and file_end != len(data):
        raise error_type(f"{path} is truncated or has trailing data")
    if string_block_policy == "within" and file_end > len(data):
        raise error_type(f"{path} is truncated")
    records = tuple(
        data[20 + index * record_size : 20 + (index + 1) * record_size]
        for index in range(count)
    )
    strings = data[records_end:file_end] if string_block_policy != "ignore" else data[records_end:]
    return WdbcFile(fields, record_size, records, strings, data)
