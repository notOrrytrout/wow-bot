#!/usr/bin/env python3
"""Build the versioned AzerothCore catalog consumed by the runtime.

The SQL/DBC builder generates world services, gathering, and source relations.
The quest-hint generator enriches the checked-in quest catalog from current
AzerothCore SQL and client DBC data. This command combines both outputs under
wow-infra ownership and records the source provenance.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    infra_catalog = repo / "crates/wow-infra/data/world-knowledge/azerothcore-catalog.json"
    legacy_catalog = repo / "crates/wow-policy/data/azerothcore-quest-hints.json"
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("azerothcore_root", type=Path)
    parser.add_argument("--dbc-dir", type=Path, required=True)
    parser.add_argument("--quest-catalog", type=Path, default=infra_catalog if infra_catalog.exists() else legacy_catalog)
    parser.add_argument("--output", type=Path, default=infra_catalog)
    args = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="wow-world-knowledge-") as temporary_dir:
        temporary = Path(temporary_dir)
        services_path = temporary / "world-services.json"
        quests_path = temporary / "quest-catalog.json"
        subprocess.run(
            [sys.executable, str(Path(__file__).with_name("build_world_knowledge.py")),
             str(args.azerothcore_root), str(services_path), "--dbc-dir", str(args.dbc_dir)],
            check=True,
        )
        subprocess.run(
            [sys.executable, str(Path(__file__).with_name("generate_quest_spell_hints.py")),
             str(args.azerothcore_root), "--dbc-dir", str(args.dbc_dir),
             "--quest-hints", str(args.quest_catalog), "--output", str(quests_path)],
            check=True,
        )
        catalog = json.loads(quests_path.read_text(encoding="utf-8"))
        services = json.loads(services_path.read_text(encoding="utf-8"))

    catalog["catalog_format_version"] = 1
    catalog["world"] = services
    catalog["provenance"] = {
        "project": "AzerothCore/azerothcore-wotlk",
        "azerothcore_commit": services.get("source", {}).get("azerothcore_commit"),
        "quest_catalog": catalog.get("source", {}),
        "world_data": services.get("source", {}),
        "note": "AzerothCore revision is null when the supplied source is not a Git checkout; source file hashes remain available.",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary_output = args.output.with_suffix(args.output.suffix + ".tmp")
    temporary_output.write_text(json.dumps(catalog, separators=(",", ":"), ensure_ascii=False), encoding="utf-8")
    temporary_output.replace(args.output)
    print(f"wrote consolidated AzerothCore World Knowledge catalog: {args.output}")
    print(f"quests={len(catalog.get('quests', {}))}, quest spell rules={len(catalog.get('quest_spell_rules', {}))}, "
          f"quest item-use rules={len(catalog.get('quest_item_use_rules', {}))}, "
          f"vendors/services={len(services.get('vendor_services', []))}, "
          f"trainers={len(services.get('trainer_services', []))}, gather nodes={len(services.get('gather_nodes', []))}")


if __name__ == "__main__":
    main()
