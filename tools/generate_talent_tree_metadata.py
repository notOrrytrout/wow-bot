#!/usr/bin/env python3
"""Generate the reviewed WotLK talent ID to class/tree map from Talent.dbc."""

import argparse
import hashlib
import json
import struct
from pathlib import Path


# TalentTab.dbc IDs and tree pages from the 3.3.5a client data. Keep this
# explicit: pet tabs and tabs for other client versions must not be guessed.
TAB_TREES = {
    161: (1, 0), 164: (1, 1), 163: (1, 2),
    382: (2, 0), 383: (2, 1), 381: (2, 2),
    361: (3, 0), 363: (3, 1), 362: (3, 2),
    182: (4, 0), 181: (4, 1), 183: (4, 2),
    201: (5, 0), 202: (5, 1), 203: (5, 2),
    398: (6, 0), 399: (6, 1), 400: (6, 2),
    261: (7, 0), 263: (7, 1), 262: (7, 2),
    81: (8, 0), 41: (8, 1), 61: (8, 2),
    302: (9, 0), 303: (9, 1), 301: (9, 2),
    283: (11, 0), 281: (11, 1), 282: (11, 2),
}


def dbc_records(path: Path, minimum_fields: int) -> list[tuple[int, ...]]:
    data = path.read_bytes()
    if len(data) < 20 or data[:4] != b"WDBC":
        raise ValueError(f"{path} has an invalid WDBC header")
    count, fields, record_size, _ = struct.unpack_from("<4I", data, 4)
    if fields < minimum_fields or record_size < fields * 4:
        raise ValueError(f"{path} has an unsupported record layout")
    records_end = 20 + count * record_size
    if records_end > len(data):
        raise ValueError(f"{path} has truncated records")
    return [
        struct.unpack_from("<" + "I" * fields, data, 20 + i * record_size)
        for i in range(count)
    ]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("dbc_dir", type=Path)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("crates/wow-state/data/wotlk-talent-trees.json"),
    )
    args = parser.parse_args()

    talent_path = args.dbc_dir / "Talent.dbc"
    tab_path = args.dbc_dir / "TalentTab.dbc"
    tab_pages = {record[0]: record[22] for record in dbc_records(tab_path, 23)}
    if any(tab_pages.get(tab_id) != tree for tab_id, (_, tree) in TAB_TREES.items()):
        raise ValueError("TalentTab.dbc tree pages do not match the reviewed 3.3.5a map")
    talents: dict[str, dict[str, int]] = {}
    counts = {(class_id, tree): 0 for class_id, tree in TAB_TREES.values()}
    for record in dbc_records(talent_path, 23):
        talent_id, tab_id = record[:2]
        if tab_id not in tab_pages or tab_id not in TAB_TREES:
            continue
        class_id, tree = TAB_TREES[tab_id]
        key = str(talent_id)
        if key in talents:
            raise ValueError(f"duplicate talent ID {talent_id}")
        talents[key] = {"class_id": class_id, "tree": tree}
        counts[(class_id, tree)] += 1
    if any(count == 0 for count in counts.values()):
        raise ValueError("Talent.dbc does not contain every player class tree")

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(
            {
                "format_version": 1,
                "source": {
                    "client_build": 12340,
                    "talent_sha256": hashlib.sha256(talent_path.read_bytes()).hexdigest(),
                    "talent_tab_sha256": hashlib.sha256(tab_path.read_bytes()).hexdigest(),
                },
                "talents": talents,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {len(talents)} talent mappings to {args.output}")


if __name__ == "__main__":
    main()
