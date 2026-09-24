#!/usr/bin/env python3
"""Build Tentacli's static WotLK world-knowledge pack from AzerothCore base SQL.

The generated pack is runtime input only: Tentacli never needs a connection to the
AzerothCore world database. Packet observations remain authoritative at runtime.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import subprocess
from collections import defaultdict
from pathlib import Path

CREATE_COLUMN_RE = re.compile(r"^\s*`([^`]+)`\s+")


def table_columns(path: Path) -> list[str]:
    columns: list[str] = []
    in_create = False
    with path.open("r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            if line.startswith("CREATE TABLE"):
                in_create = True
                continue
            if not in_create:
                continue
            if line.startswith(") ENGINE="):
                break
            match = CREATE_COLUMN_RE.match(line)
            if match:
                columns.append(match.group(1))
    if not columns:
        raise RuntimeError(f"could not discover columns in {path}")
    return columns


def parse_tuple(line: str) -> list[str | None]:
    text = line.strip()
    if not text.startswith("("):
        raise ValueError("not a tuple")
    if text.endswith(",") or text.endswith(";"):
        text = text[:-1]
    if not text.endswith(")"):
        raise ValueError("tuple does not end with ')'")
    text = text[1:-1]

    values: list[str | None] = []
    current: list[str] = []
    quoted = False
    escaped = False
    i = 0
    while i < len(text):
        ch = text[i]
        if quoted:
            if escaped:
                escapes = {"0": "\0", "b": "\b", "n": "\n", "r": "\r", "t": "\t", "Z": "\x1a"}
                current.append(escapes.get(ch, ch))
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == "'":
                # MySQL also permits doubled quotes in some dumps.
                if i + 1 < len(text) and text[i + 1] == "'":
                    current.append("'")
                    i += 1
                else:
                    quoted = False
            else:
                current.append(ch)
        else:
            if ch == "'":
                quoted = True
            elif ch == ",":
                raw = "".join(current).strip()
                values.append(None if raw.upper() == "NULL" else raw)
                current.clear()
            else:
                current.append(ch)
        i += 1
    raw = "".join(current).strip()
    values.append(None if raw.upper() == "NULL" else raw)
    return values


def rows(path: Path):
    """Yield rows from AzerothCore's mysqldump-style one-tuple-per-line base SQL."""
    with path.open("r", encoding="utf-8", errors="replace") as fh:
        in_insert = False
        for line in fh:
            if line.startswith("INSERT INTO "):
                in_insert = True
                # Current AzerothCore base dumps place VALUES on this line and data on following lines.
                continue
            if not in_insert:
                continue
            stripped = line.lstrip()
            if stripped.startswith("("):
                yield parse_tuple(stripped)
                if stripped.rstrip().endswith(";"):
                    in_insert = False
            elif stripped.startswith("UNLOCK TABLES"):
                in_insert = False


def indexes(path: Path, names: list[str]) -> dict[str, int]:
    cols = table_columns(path)
    lowered = {name.lower(): i for i, name in enumerate(cols)}
    missing = [name for name in names if name.lower() not in lowered]
    if missing:
        raise RuntimeError(f"{path.name} is missing expected columns: {missing}; columns={cols}")
    return {name: lowered[name.lower()] for name in names}


def as_int(value: str | None) -> int:
    return int(value or "0")


def as_float(value: str | None) -> float:
    return float(value or "0")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def lock_rows_from_dbc(path: Path):
    """Yield Lock.dbc rows as ID + Type[8] + Index[8] + Skill[8]."""
    data = path.read_bytes()
    if len(data) < 20:
        raise RuntimeError(f"{path} is too small to be a WDBC file")
    magic, record_count, field_count, record_size, string_size = struct.unpack_from("<4s4I", data, 0)
    if magic != b"WDBC":
        raise RuntimeError(f"{path} has invalid DBC magic {magic!r}")
    if field_count < 33 or record_size < 33 * 4:
        raise RuntimeError(
            f"{path} has an unsupported Lock.dbc layout: fields={field_count}, record_size={record_size}"
        )
    records_end = 20 + record_count * record_size
    if records_end + string_size > len(data):
        raise RuntimeError(f"{path} is truncated")
    for index in range(record_count):
        offset = 20 + index * record_size
        values = struct.unpack_from("<33I", data, offset)
        yield values[:25]


def lock_rows_from_sql(path: Path):
    """Yield Lock rows from current lock_dbc.sql or legacy lock.sql."""
    cols = table_columns(path)
    normalized = {name.lower().replace("_", ""): i for i, name in enumerate(cols)}
    expected = ["id"] + [f"type{i}" for i in range(1, 9)] + [f"index{i}" for i in range(1, 9)] + [f"skill{i}" for i in range(1, 9)]
    missing = [name for name in expected if name not in normalized]
    if missing:
        raise RuntimeError(f"{path.name} is missing expected Lock columns: {missing}; columns={cols}")
    for row in rows(path):
        yield tuple(as_int(row[normalized[name]]) for name in expected)


PROFESSION_SKILLS = {129, 164, 165, 171, 182, 185, 186, 197, 202, 333, 356, 393, 755, 773}


def dbc_records(path: Path):
    """Yield little-endian u32 fields from a WotLK WDBC file."""
    data = path.read_bytes()
    if len(data) < 20:
        raise RuntimeError(f"{path} is too small to be a WDBC file")
    magic, record_count, field_count, record_size, string_size = struct.unpack_from("<4s4I", data, 0)
    if magic != b"WDBC" or field_count == 0 or record_size != field_count * 4:
        raise RuntimeError(
            f"{path} has an unsupported WDBC layout: magic={magic!r}, fields={field_count}, record_size={record_size}"
        )
    records_end = 20 + record_count * record_size
    if records_end + string_size > len(data):
        raise RuntimeError(f"{path} is truncated")
    fmt = f"<{field_count}I"
    for index in range(record_count):
        yield struct.unpack_from(fmt, data, 20 + index * record_size)


def find_dbc(args, name: str) -> Path | None:
    candidates: list[Path] = []
    if args.dbc_dir is not None:
        candidates.append(args.dbc_dir / name)
    candidates.extend([
        args.azerothcore_root / "dbc" / name,
        args.azerothcore_root / "data" / "dbc" / name,
    ])
    return next((candidate for candidate in candidates if candidate.is_file()), None)


def profession_spell_skills(skill_line_ability_path: Path, spell_path: Path | None) -> dict[int, int]:
    """Map taught spell IDs to profession skill lines using canonical client DBC metadata."""
    result: dict[int, int] = {}
    for values in dbc_records(skill_line_ability_path):
        if len(values) < 3:
            continue
        skill = values[1]
        spell_id = values[2]
        if skill in PROFESSION_SKILLS and spell_id:
            result[spell_id] = skill

    # Profession rank spells can be SKILL_STEP effects and do not always appear
    # as an ordinary recipe relation. Mirror the runtime profession parser.
    if spell_path is not None:
        for values in dbc_records(spell_path):
            if len(values) < 113:
                continue
            spell_id = values[0]
            for effect in range(3):
                if values[71 + effect] == 44:  # SPELL_EFFECT_SKILL_STEP
                    skill = values[110 + effect]
                    if skill in PROFESSION_SKILLS and spell_id:
                        result[spell_id] = skill
    return result



def git_commit(root: Path) -> str | None:
    try:
        return subprocess.check_output(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        # Source snapshots are often supplied without the .git directory.
        # The generated file still records hashes for every SQL/DBC input.
        return None


def resolve_loot_items(
    direct_items: set[int],
    reference_ids: set[int],
    reference_rows: dict[int, list[tuple[int, int]]],
) -> set[int]:
    """Resolve nested reference_loot_template groups with cycle protection."""
    resolved = set(direct_items)
    pending = list(reference_ids)
    visited: set[int] = set()
    while pending:
        reference = pending.pop()
        if not reference or reference in visited:
            continue
        visited.add(reference)
        for item, nested_reference in reference_rows.get(reference, ()):
            if nested_reference:
                pending.append(nested_reference)
            elif item:
                resolved.add(item)
    return resolved


def runtime_spawn_key(entry: int, spawn: dict) -> tuple:
    """Identity retained by the Rust runtime; zone/area metadata is non-semantic."""
    return (
        entry,
        spawn["map_id"],
        spawn["x"],
        spawn["y"],
        spawn["z"],
        spawn["orientation"],
    )

def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("azerothcore_root", type=Path, help="path to an AzerothCore source checkout")
    parser.add_argument("output", type=Path, help="output JSON knowledge pack")
    parser.add_argument(
        "--dbc-dir",
        type=Path,
        default=None,
        help="directory that contains Lock.dbc; required when lock_dbc.sql has no rows",
    )
    args = parser.parse_args()

    sql = args.azerothcore_root / "data/sql/base/db_world"
    required = [
        "creature_queststarter.sql",
        "creature_questender.sql",
        "gameobject_queststarter.sql",
        "gameobject_questender.sql",
        "creature_template.sql",
        "creature.sql",
        "creature_loot_template.sql",
        "quest_template.sql",
        "quest_template_addon.sql",
        "gameobject.sql",
        "gameobject_template.sql",
        "gameobject_loot_template.sql",
        "fishing_loot_template.sql",
        "reference_loot_template.sql",
        "item_template.sql",
        "creature_default_trainer.sql",
        "trainer.sql",
        "trainer_spell.sql",
    ]
    for name in required:
        if not (sql / name).is_file():
            raise SystemExit(f"missing AzerothCore world SQL file: {sql / name}")

    # Quest class eligibility. AzerothCore uses a class bit mask where zero is
    # unrestricted and each class uses bit (class_id - 1). Keep this metadata in
    # the static pack so a Paladin never walks to a Hunter-only starter simply
    # because that NPC appears in creature_queststarter.
    addon_path = sql / "quest_template_addon.sql"
    qai = indexes(addon_path, ["ID", "AllowableClasses"])
    quest_allowable_classes: dict[int, int] = {}
    for row in rows(addon_path):
        quest_id = as_int(row[qai["ID"]])
        if quest_id:
            quest_allowable_classes[quest_id] = as_int(row[qai["AllowableClasses"]])

    # quest -> creature template entries for quest starters. These static public
    # relations are used only to navigate into packet-observation range; packet
    # state remains authoritative for whether an NPC currently offers the quest.
    qstarter_path = sql / "creature_queststarter.sql"
    qsi = indexes(qstarter_path, ["id", "quest"])
    quest_starters: dict[int, set[int]] = defaultdict(set)
    starter_entries: set[int] = set()
    for row in rows(qstarter_path):
        entry = as_int(row[qsi["id"]])
        quest = as_int(row[qsi["quest"]])
        if entry and quest:
            quest_starters[quest].add(entry)
            starter_entries.add(entry)

    # quest -> creature template entries for turn-ins
    qender_path = sql / "creature_questender.sql"
    qi = indexes(qender_path, ["id", "quest"])
    quest_enders: dict[int, set[int]] = defaultdict(set)
    ender_entries: set[int] = set()
    for row in rows(qender_path):
        entry = as_int(row[qi["id"]])
        quest = as_int(row[qi["quest"]])
        if entry and quest:
            quest_enders[quest].add(entry)
            ender_entries.add(entry)

    # quest -> gameobject template entries for quest starters and turn-ins.
    # These are static routing hints only; packet state remains authoritative.
    go_qstarter_path = sql / "gameobject_queststarter.sql"
    gqsi = indexes(go_qstarter_path, ["id", "quest"])
    gameobject_quest_starters: dict[int, set[int]] = defaultdict(set)
    gameobject_starter_entries: set[int] = set()
    for row in rows(go_qstarter_path):
        entry = as_int(row[gqsi["id"]])
        quest = as_int(row[gqsi["quest"]])
        if entry and quest:
            gameobject_quest_starters[quest].add(entry)
            gameobject_starter_entries.add(entry)

    go_qender_path = sql / "gameobject_questender.sql"
    gqei = indexes(go_qender_path, ["id", "quest"])
    gameobject_quest_enders: dict[int, set[int]] = defaultdict(set)
    gameobject_ender_entries: set[int] = set()
    for row in rows(go_qender_path):
        entry = as_int(row[gqei["id"]])
        quest = as_int(row[gqei["quest"]])
        if entry and quest:
            gameobject_quest_enders[quest].add(entry)
            gameobject_ender_entries.add(entry)

    # Static trainer relations. These are routing hints only. The live trainer
    # list remains authoritative for availability, prerequisites, and purchases.
    trainer_path = sql / "trainer.sql"
    tri = indexes(trainer_path, ["Id", "Type"])
    profession_trainer_ids: set[int] = set()
    trainer_types: dict[int, int] = {}
    for row in rows(trainer_path):
        trainer_id = as_int(row[tri["Id"]])
        trainer_type = as_int(row[tri["Type"]])
        if trainer_id:
            trainer_types[trainer_id] = trainer_type

    skill_line_ability_path = find_dbc(args, "SkillLineAbility.dbc")
    spell_dbc_path = find_dbc(args, "Spell.dbc")
    if skill_line_ability_path is None or spell_dbc_path is None:
        raise SystemExit(
            "Trainer generation needs SkillLineAbility.dbc and Spell.dbc. "
            "Pass --dbc-dir /path/to/dbc."
        )
    trainer_spell_skill = profession_spell_skills(skill_line_ability_path, spell_dbc_path)
    trainer_spell_path = sql / "trainer_spell.sql"
    tsi = indexes(trainer_spell_path, ["TrainerId", "SpellId"])
    trainer_skills: dict[int, set[int]] = defaultdict(set)
    for row in rows(trainer_spell_path):
        trainer_id = as_int(row[tsi["TrainerId"]])
        spell_id = as_int(row[tsi["SpellId"]])
        skill = trainer_spell_skill.get(spell_id)
        if trainer_types.get(trainer_id) == 2 and skill in PROFESSION_SKILLS:
            trainer_skills[trainer_id].add(skill)
            profession_trainer_ids.add(trainer_id)

    creature_default_trainer_path = sql / "creature_default_trainer.sql"
    cdti = indexes(creature_default_trainer_path, ["CreatureId", "TrainerId"])
    trainer_by_creature: dict[int, int] = {}
    profession_trainer_entries: set[int] = set()
    for row in rows(creature_default_trainer_path):
        entry = as_int(row[cdti["CreatureId"]])
        trainer_id = as_int(row[cdti["TrainerId"]])
        if entry and trainer_id in profession_trainer_ids:
            trainer_by_creature[entry] = trainer_id
            profession_trainer_entries.add(entry)

    # creature template names + loot-template relationship
    template_path = sql / "creature_template.sql"
    ti = indexes(template_path, ["entry", "name", "lootid", "npcflag"])
    creature_names: dict[int, str] = {}
    lootid_to_creatures: dict[int, list[int]] = defaultdict(list)
    vendor_service_flags: dict[int, tuple[bool, bool, bool]] = {}
    vendor_service_entries: set[int] = set()
    for row in rows(template_path):
        entry = as_int(row[ti["entry"]])
        if not entry:
            continue
        lootid = as_int(row[ti["lootid"]])
        if lootid:
            lootid_to_creatures[lootid].append(entry)
        npcflag = as_int(row[ti["npcflag"]])
        # AzerothCore exposes multiple specialized seller bits in addition to
        # UNIT_NPC_FLAG_VENDOR. Treat all ordinary supply vendor families as
        # sell-capable so ammo/food/poison/reagent sellers are discoverable.
        can_sell = (npcflag & 0x00000F80) != 0
        can_repair = (npcflag & 0x00001000) != 0
        can_auction = (npcflag & 0x00200000) != 0
        if can_sell or can_repair or can_auction:
            vendor_service_flags[entry] = (can_sell, can_repair, can_auction)
            vendor_service_entries.add(entry)
        if entry in ender_entries or entry in starter_entries or entry in profession_trainer_entries:
            creature_names[entry] = row[ti["name"]] or ""

    # Quest objective entries. Positive RequiredNpcOrGo values are creature
    # templates; negative values are gameobject templates in WotLK quest data.
    quest_template_path = sql / "quest_template.sql"
    qti = indexes(quest_template_path, [
        "ID",
        "RequiredNpcOrGo1", "RequiredNpcOrGo2", "RequiredNpcOrGo3", "RequiredNpcOrGo4",
        "RequiredItemId1", "RequiredItemId2", "RequiredItemId3",
        "RequiredItemId4", "RequiredItemId5", "RequiredItemId6",
    ])
    objective_creatures: set[int] = set()
    objective_gameobjects: set[int] = set()
    objective_items: set[int] = set()
    for row in rows(quest_template_path):
        for name in ("RequiredNpcOrGo1", "RequiredNpcOrGo2", "RequiredNpcOrGo3", "RequiredNpcOrGo4"):
            value = as_int(row[qti[name]])
            if value > 0:
                objective_creatures.add(value)
            elif value < 0:
                objective_gameobjects.add(-value)
        for name in (
            "RequiredItemId1", "RequiredItemId2", "RequiredItemId3",
            "RequiredItemId4", "RequiredItemId5", "RequiredItemId6",
        ):
            item_id = as_int(row[qti[name]])
            if item_id:
                objective_items.add(item_id)

    # Capture direct and referenced creature loot groups. References are resolved
    # after reference_loot_template is loaded; packet state still remains runtime
    # authority for actual loot availability.
    loot_path = sql / "creature_loot_template.sql"
    li = indexes(loot_path, ["Entry", "Item", "Reference"])
    creature_loot_rows: dict[int, tuple[set[int], set[int]]] = defaultdict(lambda: (set(), set()))
    for row in rows(loot_path):
        lootid = as_int(row[li["Entry"]])
        item = as_int(row[li["Item"]])
        reference = as_int(row[li["Reference"]])
        if not lootid:
            continue
        direct, refs = creature_loot_rows[lootid]
        if reference:
            refs.add(reference)
        elif item:
            direct.add(item)

    objective_item_source_entries: set[int] = set()

    # Retain only spawns that can participate in the quest lifecycle. This includes
    # starters, turn-ins, explicit creature objectives, and sources for items that
    # quest_template actually requires. Do not retain every creature that drops any
    # ordinary loot item; that would turn the compact reference pack into a world dump.
    # id1/id2/id3 reflect the supplied AzerothCore creature schema; older c.id
    # assumptions are avoided.
    creature_path = sql / "creature.sql"
    ci = indexes(creature_path, [
        "id1", "id2", "id3", "map", "zoneId", "areaId",
        "position_x", "position_y", "position_z", "orientation",
    ])
    spawns_by_entry: dict[int, list[dict]] = defaultdict(list)
    seen_spawn_keys: set[tuple] = set()
    relevant_spawn_entries = (
        ender_entries | starter_entries | objective_creatures | objective_item_source_entries | vendor_service_entries | profession_trainer_entries
    )
    for row in rows(creature_path):
        entries = {as_int(row[ci[name]]) for name in ("id1", "id2", "id3")}
        entries.discard(0)
        matching = entries & relevant_spawn_entries
        if not matching:
            continue
        spawn = {
            "map_id": as_int(row[ci["map"]]),
            "zone_id": as_int(row[ci["zoneId"]]),
            "area_id": as_int(row[ci["areaId"]]),
            "x": as_float(row[ci["position_x"]]),
            "y": as_float(row[ci["position_y"]]),
            "z": as_float(row[ci["position_z"]]),
            "orientation": as_float(row[ci["orientation"]]),
        }
        for entry in matching:
            key = runtime_spawn_key(entry, spawn)
            if key not in seen_spawn_keys:
                seen_spawn_keys.add(key)
                spawns_by_entry[entry].append(spawn)

    starts: list[dict] = []
    for quest_id in sorted(quest_starters):
        for entry in sorted(quest_starters[quest_id]):
            spawns = spawns_by_entry.get(entry, [])
            if not spawns:
                continue
            starts.append({
                "quest_id": quest_id,
                "giver_entry_id": entry,
                "giver_kind": "creature",
                "giver_name": creature_names.get(entry) or None,
                "allowable_classes": quest_allowable_classes.get(quest_id, 0),
                "spawns": sorted(spawns, key=lambda value: (value["map_id"], value["x"], value["y"], value["z"])),
            })

    turn_ins: list[dict] = []
    for quest_id in sorted(quest_enders):
        for entry in sorted(quest_enders[quest_id]):
            spawns = spawns_by_entry.get(entry, [])
            if not spawns:
                continue
            turn_ins.append({
                "quest_id": quest_id,
                "giver_entry_id": entry,
                "giver_kind": "creature",
                "giver_name": creature_names.get(entry) or None,
                "spawns": sorted(spawns, key=lambda value: (value["map_id"], value["x"], value["y"], value["z"])),
            })

    gameobject_path = sql / "gameobject.sql"
    gi = indexes(gameobject_path, [
        "id", "map", "zoneId", "areaId",
        "position_x", "position_y", "position_z", "orientation",
    ])
    gameobject_spawns_by_entry: dict[int, list[dict]] = defaultdict(list)
    seen_gameobject_keys: set[tuple] = set()
    for row in rows(gameobject_path):
        entry = as_int(row[gi["id"]])
        spawn = {
            "map_id": as_int(row[gi["map"]]),
            "zone_id": as_int(row[gi["zoneId"]]),
            "area_id": as_int(row[gi["areaId"]]),
            "x": as_float(row[gi["position_x"]]),
            "y": as_float(row[gi["position_y"]]),
            "z": as_float(row[gi["position_z"]]),
            "orientation": as_float(row[gi["orientation"]]),
        }
        key = runtime_spawn_key(entry, spawn)
        if key not in seen_gameobject_keys:
            seen_gameobject_keys.add(key)
            gameobject_spawns_by_entry[entry].append(spawn)

    # Lock.dbc semantics: Type == 2 means a skill key. Index identifies
    # the lock action (2=Herbalism, 3=Mining, 19=Fishing). Skill is the
    # required profession rank.
    LOCK_KEY_SKILL = 2
    LOCKTYPE_HERBALISM = 2
    LOCKTYPE_MINING = 3
    LOCKTYPE_FISHING = 19
    GAMEOBJECT_TYPE_CHEST = 3
    GAMEOBJECT_TYPE_FISHINGHOLE = 25

    lock_sql_path = next(
        (candidate for candidate in (sql / "lock_dbc.sql", sql / "lock.sql") if candidate.is_file()),
        None,
    )
    lock_rows = list(lock_rows_from_sql(lock_sql_path)) if lock_sql_path is not None else []
    lock_source_path = lock_sql_path if lock_rows else None

    if not lock_rows:
        lock_dbc_path = find_dbc(args, "Lock.dbc")
        if lock_dbc_path is None:
            raise SystemExit(
                "Gather generation needs Lock.dbc because the AzerothCore lock SQL has no data. "
                "Pass --dbc-dir /path/to/dbc."
            )
        lock_rows = list(lock_rows_from_dbc(lock_dbc_path))
        lock_source_path = lock_dbc_path

    lock_kinds: dict[int, tuple[str, int]] = {}
    for row in lock_rows:
        lock_id = row[0]
        if not lock_id:
            continue
        types = row[1:9]
        indexes_ = row[9:17]
        skills = row[17:25]
        for lock_key_type, lock_index, required_skill in zip(types, indexes_, skills):
            if lock_key_type != LOCK_KEY_SKILL:
                continue
            if lock_index == LOCKTYPE_MINING:
                lock_kinds[lock_id] = ("mining", required_skill)
                break
            if lock_index == LOCKTYPE_HERBALISM:
                lock_kinds[lock_id] = ("herbalism", required_skill)
                break
            if lock_index == LOCKTYPE_FISHING:
                lock_kinds[lock_id] = ("fishing", required_skill)
                break

    # Capture direct and referenced game-object loot groups. For chest templates
    # Data1 is the gameobject_loot_template Entry. Recursive references establish
    # possible static sources; live loot remains authoritative at runtime.
    go_loot_path = sql / "gameobject_loot_template.sql"
    gli = indexes(go_loot_path, ["Entry", "Item", "Reference"])
    gameobject_loot_rows: dict[int, tuple[set[int], set[int]]] = defaultdict(lambda: (set(), set()))
    for row in rows(go_loot_path):
        lootid = as_int(row[gli["Entry"]])
        item = as_int(row[gli["Item"]])
        reference = as_int(row[gli["Reference"]])
        if not lootid:
            continue
        direct, refs = gameobject_loot_rows[lootid]
        if reference:
            refs.add(reference)
        elif item:
            direct.add(item)

    # Fishing item classification. AzerothCore fishing loot can point at
    # reference_loot_template groups, so resolve those references recursively
    # and map the final item IDs through item_template names. This produces an
    # exact item-name index; runtime never guesses from words such as "fish".
    fishing_loot_path = sql / "fishing_loot_template.sql"
    fli = indexes(fishing_loot_path, ["Entry", "Item", "Reference"])
    fishing_item_ids: set[int] = set()
    fishing_reference_ids: set[int] = set()
    for row in rows(fishing_loot_path):
        item = as_int(row[fli["Item"]])
        reference = as_int(row[fli["Reference"]])
        if reference:
            fishing_reference_ids.add(reference)
        elif item:
            fishing_item_ids.add(item)

    reference_loot_path = sql / "reference_loot_template.sql"
    rli = indexes(reference_loot_path, ["Entry", "Item", "Reference"])
    reference_rows: dict[int, list[tuple[int, int]]] = defaultdict(list)
    for row in rows(reference_loot_path):
        entry = as_int(row[rli["Entry"]])
        if entry:
            reference_rows[entry].append(
                (as_int(row[rli["Item"]]), as_int(row[rli["Reference"]]))
            )

    fishing_item_ids = resolve_loot_items(fishing_item_ids, fishing_reference_ids, reference_rows)

    item_to_lootids: dict[int, set[int]] = defaultdict(set)
    for lootid, (direct, refs) in creature_loot_rows.items():
        for item_id in resolve_loot_items(direct, refs, reference_rows):
            item_to_lootids[item_id].add(lootid)
    item_sources: list[dict] = []
    for item_id in sorted(item_to_lootids):
        entries: set[int] = set()
        for lootid in item_to_lootids[item_id]:
            entries.update(lootid_to_creatures.get(lootid, ()))
        if entries:
            item_sources.append({"item_id": item_id, "creature_entries": sorted(entries)})

    referenced_objective_entries = {
        entry
        for record in item_sources
        if record["item_id"] in objective_items
        for entry in record["creature_entries"]
    }
    missing_entries = referenced_objective_entries - set(spawns_by_entry)
    if missing_entries:
        for row in rows(creature_path):
            entries = {as_int(row[ci[name]]) for name in ("id1", "id2", "id3")}
            entries.discard(0)
            for entry in entries & missing_entries:
                spawn = {
                    "map_id": as_int(row[ci["map"]]),
                    "zone_id": as_int(row[ci["zoneId"]]),
                    "area_id": as_int(row[ci["areaId"]]),
                    "x": as_float(row[ci["position_x"]]),
                    "y": as_float(row[ci["position_y"]]),
                    "z": as_float(row[ci["position_z"]]),
                    "orientation": as_float(row[ci["orientation"]]),
                }
                key = runtime_spawn_key(entry, spawn)
                if key not in seen_spawn_keys:
                    seen_spawn_keys.add(key)
                    spawns_by_entry[entry].append(spawn)
    objective_item_source_entries = referenced_objective_entries

    item_to_go_lootids: dict[int, set[int]] = defaultdict(set)
    for lootid, (direct, refs) in gameobject_loot_rows.items():
        for item_id in resolve_loot_items(direct, refs, reference_rows):
            item_to_go_lootids[item_id].add(lootid)

    item_template_path = sql / "item_template.sql"
    iti = indexes(item_template_path, ["entry", "name"])
    fishing_item_names: dict[int, str] = {}
    for row in rows(item_template_path):
        item_id = as_int(row[iti["entry"]])
        if item_id in fishing_item_ids:
            name = (row[iti["name"]] or "").strip()
            if name:
                fishing_item_names[item_id] = name
    fishing_items = [
        {"item_id": item_id, "name": fishing_item_names[item_id]}
        for item_id in sorted(fishing_item_names)
    ]

    got_path = sql / "gameobject_template.sql"
    goti = indexes(got_path, ["entry", "type", "name", "Data0", "Data1"])
    lootid_to_gameobjects: dict[int, set[int]] = defaultdict(set)
    gameobject_names: dict[int, str] = {}
    gather_nodes = []
    for row in rows(got_path):
        entry = as_int(row[goti["entry"]])
        go_type = as_int(row[goti["type"]])
        name = row[goti["name"]] or ""
        if entry in gameobject_starter_entries or entry in gameobject_ender_entries:
            gameobject_names[entry] = name
        lock_id = as_int(row[goti["Data0"]])
        if go_type == GAMEOBJECT_TYPE_CHEST:
            loot_id = as_int(row[goti["Data1"]])
            if loot_id:
                lootid_to_gameobjects[loot_id].add(entry)
        if not entry or not name.strip():
            continue
        if go_type not in (GAMEOBJECT_TYPE_CHEST, GAMEOBJECT_TYPE_FISHINGHOLE):
            continue
        if go_type == GAMEOBJECT_TYPE_FISHINGHOLE:
            kind, required_skill = ("fishing", 0)
        else:
            kind_info = lock_kinds.get(lock_id)
            if not kind_info:
                continue
            kind, required_skill = kind_info
        spawns = gameobject_spawns_by_entry.get(entry)
        if not spawns:
            continue
        gather_nodes.append({
            "entry_id": entry,
            "name": name,
            "kind": kind,
            "required_skill": required_skill,
            "spawns": sorted(spawns, key=lambda value: (value["map_id"], value["x"], value["y"], value["z"])),
        })
    gather_nodes.sort(key=lambda value: (value["kind"], value["name"], value["entry_id"]))

    for quest_id in sorted(gameobject_quest_starters):
        for entry in sorted(gameobject_quest_starters[quest_id]):
            spawns = gameobject_spawns_by_entry.get(entry, [])
            if spawns:
                starts.append({
                    "quest_id": quest_id,
                    "giver_entry_id": entry,
                    "giver_kind": "game_object",
                    "giver_name": gameobject_names.get(entry) or None,
                    "allowable_classes": quest_allowable_classes.get(quest_id, 0),
                    "spawns": sorted(spawns, key=lambda value: (value["map_id"], value["x"], value["y"], value["z"])),
                })

    for quest_id in sorted(gameobject_quest_enders):
        for entry in sorted(gameobject_quest_enders[quest_id]):
            spawns = gameobject_spawns_by_entry.get(entry, [])
            if spawns:
                turn_ins.append({
                    "quest_id": quest_id,
                    "giver_entry_id": entry,
                    "giver_kind": "game_object",
                    "giver_name": gameobject_names.get(entry) or None,
                    "spawns": sorted(spawns, key=lambda value: (value["map_id"], value["x"], value["y"], value["z"])),
                })
    starts.sort(key=lambda value: (value["quest_id"], value["giver_kind"], value["giver_entry_id"]))
    turn_ins.sort(key=lambda value: (value["quest_id"], value["giver_kind"], value["giver_entry_id"]))

    gameobject_item_sources: list[dict] = []
    for item_id in sorted(item_to_go_lootids):
        entries: set[int] = set()
        for lootid in item_to_go_lootids[item_id]:
            entries.update(lootid_to_gameobjects.get(lootid, ()))
        if entries:
            gameobject_item_sources.append({
                "item_id": item_id,
                "gameobject_entries": sorted(entries),
            })
    objective_item_gameobject_entries = {
        entry
        for record in gameobject_item_sources
        if record["item_id"] in objective_items
        for entry in record["gameobject_entries"]
    }

    objective_creature_entries = objective_creatures | objective_item_source_entries
    creature_spawns = [
        {
            "entry_id": entry,
            "spawns": sorted(spawns_by_entry[entry], key=lambda value: (value["map_id"], value["x"], value["y"], value["z"])),
        }
        for entry in sorted(objective_creature_entries)
        if spawns_by_entry.get(entry)
    ]
    gameobject_spawns = [
        {
            "entry_id": entry,
            "spawns": sorted(gameobject_spawns_by_entry[entry], key=lambda value: (value["map_id"], value["x"], value["y"], value["z"])),
        }
        for entry in sorted(objective_gameobjects | objective_item_gameobject_entries)
        if gameobject_spawns_by_entry.get(entry)
    ]
    vendor_services = [
        {
            "entry_id": entry,
            "can_sell": vendor_service_flags[entry][0],
            "can_repair": vendor_service_flags[entry][1],
            "can_auction": vendor_service_flags[entry][2],
            "spawns": sorted(spawns_by_entry[entry], key=lambda value: (value["map_id"], value["x"], value["y"], value["z"])),
        }
        for entry in sorted(vendor_service_entries)
        if spawns_by_entry.get(entry)
    ]

    trainer_services = [
        {
            "entry_id": entry,
            "trainer_id": trainer_by_creature[entry],
            "name": creature_names.get(entry) or None,
            "skills": sorted(trainer_skills[trainer_by_creature[entry]]),
            "spawns": sorted(spawns_by_entry[entry], key=lambda value: (value["map_id"], value["x"], value["y"], value["z"])),
        }
        for entry in sorted(profession_trainer_entries)
        if spawns_by_entry.get(entry) and trainer_skills.get(trainer_by_creature.get(entry, 0))
    ]

    if not trainer_services:
        raise SystemExit(
            "generated zero profession trainer services; refusing to write a trainer-capable world-knowledge pack"
        )

    if not gather_nodes:
        raise SystemExit(
            "generated zero gather nodes; refusing to write a Gather-capable world-knowledge pack"
        )

    source_files = [sql / name for name in required]
    if lock_sql_path is not None:
        source_files.append(lock_sql_path)
    payload = {
        "format_version": 3,
        "source": {
            "project": "AzerothCore/azerothcore-wotlk",
            "source_layout": "data/sql/base/db_world",
            "azerothcore_commit": git_commit(args.azerothcore_root),
            "generator": {"name": "wow-bot-build-world-knowledge", "schema_version": 1},
            "files": {path.name: sha256(path) for path in source_files},
            "gather_lock_source": {
                "name": lock_source_path.name,
                "sha256": sha256(lock_source_path),
            },
            "trainer_skill_sources": {
                "SkillLineAbility.dbc": sha256(skill_line_ability_path),
                "Spell.dbc": sha256(spell_dbc_path),
            },
        },
        "quest_starts": starts,
        "quest_turn_ins": turn_ins,
        "quest_item_creature_sources": item_sources,
        "quest_item_gameobject_sources": gameobject_item_sources,
        "creature_spawns": creature_spawns,
        "gameobject_spawns": gameobject_spawns,
        "vendor_services": vendor_services,
        "trainer_services": trainer_services,
        "gather_nodes": gather_nodes,
        "fishing_items": fishing_items,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, separators=(",", ":"), ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {args.output}")
    print(f"quest starter relations with spawns: {len(starts)}")
    print(f"quest turn-in relations with spawns: {len(turn_ins)}")
    print(f"quest item source records: {len(item_sources)}")
    print(f"quest item game-object source records: {len(gameobject_item_sources)}")
    print(f"quest objective creature spawn records: {len(creature_spawns)}")
    print(f"quest objective gameobject spawn records: {len(gameobject_spawns)}")
    print(f"vendor/repair/auction service spawn records: {len(vendor_services)}")
    print(f"profession trainer service spawn records: {len(trainer_services)}")
    print(f"gather node templates with spawns: {len(gather_nodes)}")
    print(f"fishing item classifications: {len(fishing_items)}")


if __name__ == "__main__":
    main()
