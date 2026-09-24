#!/usr/bin/env python3
"""Expand reviewed WotLK combat priority anchors to all known spell ranks."""

import argparse
import hashlib
import json
import struct
from pathlib import Path

def read_dbc(path: Path) -> tuple[list[tuple[int, ...]], bytes]:
    data = path.read_bytes()
    if len(data) < 20 or data[:4] != b"WDBC":
        raise ValueError(f"{path} has an invalid WDBC header")
    count, fields, size, string_size = struct.unpack_from("<4I", data, 4)
    records_end = 20 + count * size
    if size < fields * 4 or records_end + string_size != len(data):
        raise ValueError(f"{path} has an unsupported or truncated DBC layout")
    records = [
        struct.unpack_from("<" + "I" * fields, data, 20 + index * size)
        for index in range(count)
    ]
    return records, data[records_end:]


def generate(dbc_dir: Path, seed_path: Path, catalog_path: Path) -> dict:
    spell_path = dbc_dir / "Spell.dbc"
    records, _ = read_dbc(spell_path)
    by_id = {record[0] for record in records if record[0]}
    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    family_by_spell = {spell["id"]: spell["family_id"] for spell in catalog["spells"]}
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
                    if spell_id not in by_id:
                        raise ValueError(f"priority spell {spell_id} is absent from Spell.dbc")
                    family_id = family_by_spell.get(spell_id)
                    if family_id is None:
                        raise ValueError(
                            f"priority spell {spell_id} is absent from the player class catalogue"
                        )
                    priorities.append(family_id)
                power_policies[power_type] = priorities
            trees[tree] = {"power_policies": power_policies}
        classes[class_id] = {"trees": trees}

    return {
        "format_version": 2,
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
        "--spell-catalog",
        type=Path,
        default=root / "crates/wow-policy/data/spell-catalog.json",
    )
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
    result = generate(args.dbc_dir, args.seed, args.spell_catalog)
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
