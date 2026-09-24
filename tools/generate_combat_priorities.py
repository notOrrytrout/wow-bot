#!/usr/bin/env python3
"""Expand reviewed WotLK combat priority anchors to all known spell ranks."""

import argparse
import hashlib
import json
import re
import struct
from pathlib import Path

SPELL_NAME_FIELD = 136
SPELL_RANK_FIELD = 153


def read_dbc(path: Path) -> tuple[list[tuple[int, ...]], bytes]:
    data = path.read_bytes()
    if len(data) < 20 or data[:4] != b"WDBC":
        raise ValueError(f"{path} has an invalid WDBC header")
    count, fields, size, string_size = struct.unpack_from("<4I", data, 4)
    records_end = 20 + count * size
    if fields <= SPELL_RANK_FIELD or size < fields * 4 or records_end + string_size != len(data):
        raise ValueError(f"{path} has an unsupported or truncated DBC layout")
    records = [
        struct.unpack_from("<" + "I" * fields, data, 20 + index * size)
        for index in range(count)
    ]
    return records, data[records_end:]


def dbc_string(strings: bytes, offset: int) -> str:
    if offset >= len(strings):
        return ""
    return strings[offset:].split(b"\0", 1)[0].decode("utf-8", errors="replace")


def generate(dbc_dir: Path, seed_path: Path) -> dict:
    spell_path = dbc_dir / "Spell.dbc"
    records, strings = read_dbc(spell_path)
    by_id = {record[0]: record for record in records if record[0]}
    by_name: dict[str, list[tuple[int, int]]] = {}
    for record in records:
        spell_id = record[0]
        name = dbc_string(strings, record[SPELL_NAME_FIELD])
        if spell_id and name:
            rank_text = dbc_string(strings, record[SPELL_RANK_FIELD])
            rank_match = re.fullmatch(r"Rank (\d+)", rank_text)
            rank = int(rank_match.group(1)) if rank_match else 0
            by_name.setdefault(name, []).append((rank, spell_id))

    seed = json.loads(seed_path.read_text(encoding="utf-8"))
    classes = {}
    for class_id, class_seed in seed["classes"].items():
        trees = {}
        for tree, tree_seed in class_seed["trees"].items():
            profiles = {
                str(tree_seed["power_type"]): tree_seed["priorities"],
            }
            if "alternate_power_type" in tree_seed:
                profiles[str(tree_seed["alternate_power_type"])] = tree_seed[
                    "alternate_priorities"
                ]
            power_policies = {}
            for power_type, anchors in profiles.items():
                priorities = []
                for anchor in anchors:
                    if len(anchor) != 1:
                        raise ValueError(
                            "priority seed entries must each contain one reviewed spell ID"
                        )
                    spell_id = anchor[0]
                    record = by_id.get(spell_id)
                    if record is None:
                        raise ValueError(f"priority spell {spell_id} is absent from Spell.dbc")
                    name = dbc_string(strings, record[SPELL_NAME_FIELD])
                    ranked = sorted(by_name[name], reverse=True)
                    spells = [candidate for _, candidate in ranked]
                    if spell_id not in spells:
                        raise ValueError(
                            f"priority spell {spell_id} did not resolve to its rank family"
                        )
                    priorities.append({"name": name, "spells": spells})
                power_policies[power_type] = priorities
            trees[tree] = {"power_policies": power_policies}
        classes[class_id] = {"trees": trees}

    return {
        "format_version": 1,
        "source": {
            "client_build": 12340,
            "spell_sha256": hashlib.sha256(spell_path.read_bytes()).hexdigest(),
        },
        "classes": classes,
    }


def main() -> None:
    root = Path(__file__).parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dbc-dir", type=Path, required=True)
    parser.add_argument(
        "--seed",
        type=Path,
        default=root / "crates/wow-policy/data/combat-priorities-seed.json",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=root / "crates/wow-policy/data/combat-priorities.json",
    )
    args = parser.parse_args()
    result = generate(args.dbc_dir, args.seed)
    args.output.write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf-8")
    family_count = sum(
        len(priorities)
        for policy in result["classes"].values()
        for tree in policy["trees"].values()
        for priorities in tree["power_policies"].values()
    )
    print(f"wrote {family_count} combat priority families for {len(result['classes'])} classes")


if __name__ == "__main__":
    main()
