#!/usr/bin/env python3
"""Generate the 3.3.5a spell range catalog from Spell.dbc and SpellRange.dbc."""
from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path


SPELL_RANGE_INDEX_FIELD = 46
SPELL_ATTRIBUTES_FIELD = 4
SPELL_EFFECT_APPLY_AURA_FIELD = 95
ONLY_STEALTHED_ATTRIBUTE = 0x00020000
STEALTH_AURA_TYPE = 16


def read_dbc(path: Path) -> tuple[int, int, list[bytes]]:
    data = path.read_bytes()
    if len(data) < 20:
        raise RuntimeError(f"{path} is too small to be a WDBC file")
    magic, record_count, field_count, record_size, string_size = struct.unpack_from(
        "<4s4I", data
    )
    records_end = 20 + record_count * record_size
    if magic != b"WDBC" or field_count * 4 > record_size:
        raise RuntimeError(f"{path} has an unsupported WDBC layout")
    if records_end + string_size != len(data):
        raise RuntimeError(f"{path} is truncated or has trailing data")
    records = [
        data[20 + row * record_size : 20 + (row + 1) * record_size]
        for row in range(record_count)
    ]
    return field_count, record_size, records


def generate(dbc_dir: Path) -> dict:
    spell_path = dbc_dir / "Spell.dbc"
    range_path = dbc_dir / "SpellRange.dbc"
    for path in (spell_path, range_path):
        if not path.is_file():
            raise FileNotFoundError(f"missing spell range source: {path}")

    spell_fields, _, spell_records = read_dbc(spell_path)
    range_fields, _, range_records = read_dbc(range_path)
    if spell_fields <= SPELL_RANGE_INDEX_FIELD or range_fields < 6:
        raise RuntimeError("DBC files do not match the expected 3.3.5a layouts")

    range_rows: dict[int, dict[str, float]] = {}
    for row in range_records:
        range_id = struct.unpack_from("<I", row, 0)[0]
        hostile_min, friendly_min, hostile_max, friendly_max = struct.unpack_from(
            "<ffff", row, 4
        )
        range_rows[range_id] = {
            "hostile_min": hostile_min,
            "friendly_min": friendly_min,
            "hostile_max": hostile_max,
            "friendly_max": friendly_max,
        }

    spell_range_indices = []
    stealth_required_spells = []
    stealth_aura_spells = []
    for row in spell_records:
        spell_id = struct.unpack_from("<I", row, 0)[0]
        range_id = struct.unpack_from("<I", row, SPELL_RANGE_INDEX_FIELD * 4)[0]
        if spell_id and range_id in range_rows:
            spell_range_indices.append((spell_id, range_id))
        if not spell_id:
            continue
        attributes = struct.unpack_from("<I", row, SPELL_ATTRIBUTES_FIELD * 4)[0]
        if attributes & ONLY_STEALTHED_ATTRIBUTE:
            stealth_required_spells.append(spell_id)
        aura_types = struct.unpack_from("<III", row, SPELL_EFFECT_APPLY_AURA_FIELD * 4)
        if STEALTH_AURA_TYPE in aura_types:
            stealth_aura_spells.append(spell_id)
    spell_range_indices.sort()
    stealth_required_spells.sort()
    stealth_aura_spells.sort()

    return {
        "format_version": 1,
        "source": {
            "client_build": 12340,
            "spell_sha256": hashlib.sha256(spell_path.read_bytes()).hexdigest(),
            "spell_range_sha256": hashlib.sha256(range_path.read_bytes()).hexdigest(),
        },
        "spell_range_indices": spell_range_indices,
        "ranges": range_rows,
        "stealth_required_spells": stealth_required_spells,
        "stealth_aura_spells": stealth_aura_spells,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dbc-dir", type=Path, required=True)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).parents[1] / "crates/wow-policy/data/spell-ranges.json",
    )
    args = parser.parse_args()
    catalog = generate(args.dbc_dir)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(catalog, separators=(",", ":")) + "\n")
    print(f"wrote {len(catalog['spell_range_indices'])} spell range entries to {args.output}")


if __name__ == "__main__":
    main()
