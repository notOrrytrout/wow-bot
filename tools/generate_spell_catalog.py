#!/usr/bin/env python3
"""Generate shared player-class spell semantics from WotLK 3.3.5a DBC files."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
from pathlib import Path

BUILD = 12340
CLASS_IDS = (1, 2, 3, 4, 5, 6, 7, 8, 9, 11)
ONLY_STEALTHED_ATTRIBUTE = 0x00020000
STEALTH_AURA_TYPE = 16
PET_LIFECYCLE_EFFECTS = {55, 56, 57, 101, 102, 109}
DIRECT_DAMAGE_EFFECTS = {1, 2, 7, 8, 9, 17, 31, 58, 62}
SPELL_EFFECT_APPLY_AURA = 6
SPELL_AURA_PERIODIC_DAMAGE = 3
SPELL_AURA_PERIODIC_LEECH = 53

# WotLK Spell.dbc offsets are reviewed against AzerothCore DBCStructure.h.
FIELDS = {
    "id": 0,
    "attributes": 4,
    "attributes_ex": 5,
    "attributes_ex2": 6,
    "attributes_ex3": 7,
    "attributes_ex4": 8,
    "attributes_ex5": 9,
    "attributes_ex6": 10,
    "attributes_ex7": 11,
    "stance_mask": 12,
    "stance_exclude": 14,
    "targets": 16,
    "target_creature_type": 17,
    "spell_focus": 18,
    "facing_flags": 19,
    "caster_aura_state": 20,
    "target_aura_state": 21,
    "caster_aura_state_not": 22,
    "target_aura_state_not": 23,
    "caster_aura_spell": 24,
    "target_aura_spell": 25,
    "exclude_caster_aura_spell": 26,
    "exclude_target_aura_spell": 27,
    "cast_time_index": 28,
    "cooldown_ms": 29,
    "category_cooldown_ms": 30,
    "start_recovery_category": 205,
    "start_recovery_ms": 206,
    "duration_index": 40,
    "power_type": 41,
    "base_cost": 42,
    "cost_per_level": 43,
    "cost_per_second": 44,
    "cost_per_second_per_level": 45,
    "range_index": 46,
    "reagent_start": 52,
    "reagent_count_start": 60,
    "equipped_item_class": 68,
    "equipped_item_subclass_mask": 69,
    "equipped_item_inventory_mask": 70,
    "effect_start": 71,
    "name_offset": 136,
    "rank_offset": 153,
    "cost_percent": 204,
    "spell_family": 208,
    "family_flags_start": 209,
    "school_mask": 225,
    "rune_cost_id": 226,
    "effect_aura_start": 95,
    "effect_base_points_start": 80,
    "effect_misc_start": 110,
    "effect_family_flags_start": 122,
}

SPELL_FIELDS = 234


def read_dbc(path: Path) -> tuple[list[tuple[int, ...]], bytes, bytes]:
    data = path.read_bytes()
    if len(data) < 20:
        raise ValueError(f"{path} is too small to be a WDBC file")
    magic, count, fields, row_size, string_size = struct.unpack_from("<4s4I", data)
    end = 20 + count * row_size
    if magic != b"WDBC" or fields * 4 > row_size or end + string_size != len(data):
        raise ValueError(f"{path} has an invalid or truncated WDBC layout")
    records = [
        struct.unpack_from("<" + "I" * fields, data, 20 + i * row_size)
        for i in range(count)
    ]
    return records, data[end:], data


def dbc_string(strings: bytes, offset: int) -> str:
    if offset >= len(strings):
        return ""
    return strings[offset:].split(b"\0", 1)[0].decode("utf-8", errors="replace")


def indexed_rows(path: Path) -> dict[int, tuple[int, ...]]:
    rows, _, _ = read_dbc(path)
    return {row[0]: row for row in rows if row and row[0]}


def float_value(bits: int) -> float:
    return struct.unpack("<f", struct.pack("<I", bits))[0]


def signed_value(bits: int) -> int:
    return struct.unpack("<i", struct.pack("<I", bits))[0]


def attack_spell_flags(row: tuple[int, ...]) -> tuple[bool, bool]:
    """Match the old bot's DBC rule for offensive and damage-over-time spells."""
    attack = False
    dot = False
    for effect in range(3):
        effect_kind = row[FIELDS["effect_start"] + effect]
        if effect_kind in PET_LIFECYCLE_EFFECTS:
            return False, False
        if effect_kind in DIRECT_DAMAGE_EFFECTS:
            attack = True
        if effect_kind == SPELL_EFFECT_APPLY_AURA:
            aura = row[FIELDS["effect_aura_start"] + effect]
            if aura in (SPELL_AURA_PERIODIC_DAMAGE, SPELL_AURA_PERIODIC_LEECH):
                attack = True
                dot = True
    return attack, dot


def generate(dbc_dir: Path) -> dict:
    required = (
        "Spell.dbc", "SpellRange.dbc", "SpellCastTimes.dbc", "SpellDuration.dbc",
        "SpellRuneCost.dbc", "SkillLineAbility.dbc", "Talent.dbc", "TalentTab.dbc",
        "Item.dbc",
        "SpellShapeshiftForm.dbc",
    )
    paths = {name: dbc_dir / name for name in required}
    for path in paths.values():
        if not path.is_file():
            raise FileNotFoundError(f"missing spell catalogue source: {path}")

    spell_rows, spell_strings, spell_bytes = read_dbc(paths["Spell.dbc"])
    if len(spell_rows[0]) != SPELL_FIELDS:
        raise ValueError(f"Spell.dbc must have {SPELL_FIELDS} fields")
    ranges = indexed_rows(paths["SpellRange.dbc"])
    cast_times = indexed_rows(paths["SpellCastTimes.dbc"])
    durations = indexed_rows(paths["SpellDuration.dbc"])
    rune_costs = indexed_rows(paths["SpellRuneCost.dbc"])
    item_rows = indexed_rows(paths["Item.dbc"])
    shapeshift_forms = indexed_rows(paths["SpellShapeshiftForm.dbc"])
    spell_by_id = {row[0]: row for row in spell_rows if row[0]}

    # Get class spell IDs from learned abilities and all reviewed talent ranks.
    class_spells: dict[int, set[int]] = {class_id: set() for class_id in CLASS_IDS}
    ability_rows, _, _ = read_dbc(paths["SkillLineAbility.dbc"])
    for row in ability_rows:
        spell_id, class_mask = row[2], row[4]
        for class_id in CLASS_IDS:
            if class_mask & (1 << (class_id - 1)) and spell_id:
                class_spells[class_id].add(spell_id)

    talent_tabs = indexed_rows(paths["TalentTab.dbc"])
    tab_classes = {
        tab_id: [class_id for class_id in CLASS_IDS if row[20] & (1 << (class_id - 1))]
        for tab_id, row in talent_tabs.items()
    }
    talents, _, _ = read_dbc(paths["Talent.dbc"])
    for row in talents:
        classes = tab_classes.get(row[1], [])
        for spell_id in row[4:9]:
            if spell_id in spell_by_id:
                for class_id in classes:
                    class_spells[class_id].add(spell_id)

    # Include every rank in a class spell's same-name rank family.
    by_name: dict[str, list[int]] = {}
    for spell_id, row in spell_by_id.items():
        name = dbc_string(spell_strings, row[FIELDS["name_offset"]])
        if name:
            by_name.setdefault(name, []).append(spell_id)
    for class_id, spell_ids in class_spells.items():
        for spell_id in tuple(spell_ids):
            row = spell_by_id.get(spell_id)
            if row is not None:
                name = dbc_string(spell_strings, row[FIELDS["name_offset"]])
                spell_ids.update(by_name.get(name, ()))

    for spell_ids in class_spells.values():
        spell_ids.intersection_update(spell_by_id)
    selected = set().union(*class_spells.values())
    family_members: dict[str, list[int]] = {}
    for spell_id in selected:
        row = spell_by_id[spell_id]
        spell_name = dbc_string(spell_strings, row[FIELDS["name_offset"]])
        if spell_name:
            family_members.setdefault(spell_name, []).append(spell_id)
    family_ids = {
        name: min(spell_ids) for name, spell_ids in family_members.items()
    }
    ordered_families = {}
    for name, spell_ids in family_members.items():
        def rank_number(spell_id: int) -> int:
            rank = dbc_string(spell_strings, spell_by_id[spell_id][FIELDS["rank_offset"]])
            match = re.fullmatch(r"Rank (\d+)", rank)
            return int(match.group(1)) if match else 0
        ordered_families[family_ids[name]] = sorted(
            spell_ids, key=lambda spell_id: (rank_number(spell_id), spell_id), reverse=True
        )
    records = []
    stealth_required_spells = []
    for spell_id in sorted(selected):
        row = spell_by_id.get(spell_id)
        if row is None:
            continue
        value = lambda key: row[FIELDS[key]]
        name = dbc_string(spell_strings, value("name_offset"))
        if value("attributes") & ONLY_STEALTHED_ATTRIBUTE:
            stealth_required_spells.append(spell_id)
        rank = dbc_string(spell_strings, value("rank_offset"))
        attack_spell, damage_over_time = attack_spell_flags(row)
        range_row = ranges.get(value("range_index"))
        cast_row = cast_times.get(value("cast_time_index"))
        duration_row = durations.get(value("duration_index"))
        rune_row = rune_costs.get(value("rune_cost_id"))
        if value("range_index") and range_row is None:
            raise ValueError(f"spell {spell_id} refers to missing range {value('range_index')}")
        if value("cast_time_index") and cast_row is None:
            raise ValueError(f"spell {spell_id} refers to missing cast time {value('cast_time_index')}")
        if value("duration_index") and duration_row is None:
            raise ValueError(f"spell {spell_id} refers to missing duration {value('duration_index')}")
        if value("rune_cost_id") and rune_row is None:
            raise ValueError(f"spell {spell_id} refers to missing rune cost {value('rune_cost_id')}")
        records.append({
            "id": spell_id,
            "name": name,
            "rank": rank,
            "family_id": family_ids.get(name, spell_id),
            "classes": [class_id for class_id in CLASS_IDS if spell_id in class_spells[class_id]],
            "power_type": signed_value(value("power_type")),
            "cost": {
                "base": value("base_cost"),
                "per_level": value("cost_per_level"),
                "percent": value("cost_percent"),
                "per_second": value("cost_per_second"),
                "per_second_per_level": value("cost_per_second_per_level"),
                "use_all_power": bool(value("attributes_ex") & 0x00000002),
            },
            "cooldown_ms": value("cooldown_ms"),
            "category_cooldown_ms": value("category_cooldown_ms"),
            "global_cooldown": {
                "category": value("start_recovery_category"),
                "duration_ms": value("start_recovery_ms"),
            },
            "cast_time_ms": (cast_row[1] if cast_row else (0 if value("cast_time_index") == 0 else None)),
            "duration_ms": (signed_value(duration_row[1]) if duration_row else (0 if value("duration_index") == 0 else None)),
            "range": ({
                "hostile_min": float_value(range_row[1]), "friendly_min": float_value(range_row[2]),
                "hostile_max": float_value(range_row[3]), "friendly_max": float_value(range_row[4]),
                "flags": range_row[5],
            } if range_row else None),
            "runes": ({
                "blood": rune_row[1], "frost": rune_row[2],
                "unholy": rune_row[3], "runic_power_gain": rune_row[4],
            } if rune_row else None),
            "reagents": [
                {"item": signed_value(row[FIELDS["reagent_start"] + i]),
                 "count": row[FIELDS["reagent_count_start"] + i]}
                for i in range(8) if row[FIELDS["reagent_start"] + i]
            ],
            "equipment": {
                "class": signed_value(row[FIELDS["equipped_item_class"]]),
                "subclass_mask": signed_value(row[FIELDS["equipped_item_subclass_mask"]]),
                "inventory_mask": signed_value(row[FIELDS["equipped_item_inventory_mask"]]),
            },
            "requirements": {
                key: value(key) for key in (
                    "stance_mask", "stance_exclude", "targets", "target_creature_type",
                    "spell_focus", "facing_flags", "caster_aura_state", "target_aura_state",
                    "caster_aura_state_not", "target_aura_state_not", "caster_aura_spell",
                    "target_aura_spell", "exclude_caster_aura_spell", "exclude_target_aura_spell",
                    "attributes", "attributes_ex", "attributes_ex2", "attributes_ex3",
                    "attributes_ex4", "attributes_ex5", "attributes_ex6", "attributes_ex7",
                )
            },
            "spell_family": value("spell_family"),
            "family_flags": list(row[FIELDS["family_flags_start"]:FIELDS["family_flags_start"] + 3]),
            "requires_combo_points": bool(value("attributes_ex") & 0x00500000),
            "cost_spell_modifiers": [
                {
                    "aura_type": row[FIELDS["effect_aura_start"] + effect],
                    "base_points": signed_value(row[FIELDS["effect_base_points_start"] + effect]),
                    "family_flags": list(row[
                        FIELDS["effect_family_flags_start"] + effect * 3:
                        FIELDS["effect_family_flags_start"] + effect * 3 + 3
                    ]),
                    "percent": row[FIELDS["effect_aura_start"] + effect] == 108,
                }
                for effect in range(3)
                if row[FIELDS["effect_aura_start"] + effect] in (107, 108)
                and signed_value(row[FIELDS["effect_misc_start"] + effect]) == 14
            ],
            "school_mask": value("school_mask"),
            "attack_spell": attack_spell,
            "damage_over_time": damage_over_time,
        })

    stealth_aura_spells = sorted(
        row[0] for row in spell_rows
        if row[0] and STEALTH_AURA_TYPE in row[95:98]
    )
    if not records:
        raise ValueError("no class spells found in the supplied DBC files")
    return {
        "format_version": 2,
        "source": {
            "client_build": BUILD,
            "sha256": {name: hashlib.sha256(path.read_bytes()).hexdigest()
                       for name, path in paths.items()},
        },
        "classes": list(CLASS_IDS),
        "spells": records,
        "spell_families": ordered_families,
        "items": {
            item_id: {"class": row[1], "subclass": row[2], "inventory_type": row[6]}
            for item_id, row in item_rows.items()
        },
        "shapeshift_forms": {
            form_id: {"flags": row[19], "attack_speed_ms": row[22]}
            for form_id, row in shapeshift_forms.items()
        },
        "stealth_required_spells": stealth_required_spells,
        "stealth_aura_spells": stealth_aura_spells,
    }


def main() -> None:
    root = Path(__file__).parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dbc-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path,
                        default=root / "crates/wow-policy/data/spell-catalog.json")
    args = parser.parse_args()
    catalog = generate(args.dbc_dir)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(catalog, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"wrote metadata for {len(catalog['spells'])} player class spells to {args.output}")


if __name__ == "__main__":
    main()
