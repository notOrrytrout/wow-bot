#!/usr/bin/env python3
"""Generate quest spell rules from AzerothCore SQL and client Spell.dbc.

This writes only the generated ``quest_spell_rules`` field in the existing
quest-hints catalog. It uses objective text to connect named spells to quest
targets. The runtime still requires observed spellbook and live target state.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import struct
from collections import defaultdict
from pathlib import Path


_BUILDER_PATH = Path(__file__).with_name("build_world_knowledge.py")
_SPEC = importlib.util.spec_from_file_location("world_knowledge_builder", _BUILDER_PATH)
if _SPEC is None or _SPEC.loader is None:
    raise RuntimeError(f"cannot load SQL helpers from {_BUILDER_PATH}")
_BUILDER = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_BUILDER)

ACTION_PREFIX = re.compile(
    r"\b(use|cast|apply|administer|heal|cure|bless|inoculat\w*|resurrect|"
    r"charge|cleanse|dispel|revive|poison|extinguish|deactivate|activate)"
    r"(?:\s+[\w'-]+){0,2}\s*$",
    re.IGNORECASE,
)
SPELL_TARGET_SUFFIX = re.compile(r"^\s*(?:[\w'-]+\s+){0,4}(on|at|against|upon)\b", re.IGNORECASE)
ITEM_TARGET_SUFFIX = re.compile(r"^\s*(on|at|against|upon)\b", re.IGNORECASE)


def spell_names_from_dbc(dbc_path: Path, schema_path: Path) -> dict[str, tuple[str, list[int]]]:
    """Read spell IDs and English names using the matching AzerothCore DBC schema."""
    data = dbc_path.read_bytes()
    if len(data) < 20:
        raise RuntimeError(f"{dbc_path} is too small to be a WDBC file")
    magic, record_count, field_count, record_size, string_size = struct.unpack_from("<4s4I", data, 0)
    if magic != b"WDBC" or record_size != field_count * 4:
        raise RuntimeError(f"{dbc_path} has an unsupported WDBC layout")
    columns = _BUILDER.table_columns(schema_path)
    name_index = next((i for i, name in enumerate(columns) if name.lower() == "name_lang_enus"), None)
    if name_index is None or name_index >= field_count:
        raise RuntimeError(f"{schema_path} has no compatible Name_Lang_enUS field")
    strings_at = 20 + record_count * record_size
    strings = data[strings_at:strings_at + string_size]
    if len(strings) != string_size:
        raise RuntimeError(f"{dbc_path} is truncated")

    names: dict[str, set[int]] = defaultdict(set)
    display_names: dict[str, str] = {}
    for row in range(record_count):
        offset = 20 + row * record_size
        spell_id = struct.unpack_from("<I", data, offset)[0]
        string_offset = struct.unpack_from("<I", data, offset + name_index * 4)[0]
        if not spell_id or string_offset >= len(strings):
            continue
        end = strings.find(b"\0", string_offset)
        if end < 0:
            continue
        name = strings[string_offset:end].decode("utf-8", errors="replace").strip()
        if name:
            key = name.casefold()
            names[key].add(spell_id)
            display_names.setdefault(key, name)
    return {name: (display_names[name], sorted(ids)) for name, ids in names.items()}


def _near_action(text: str, start: int, end: int) -> bool:
    sentence_start = max(text.rfind(mark, 0, start) for mark in ".!?;,") + 1
    sentence_end_candidates = [text.find(mark, end) for mark in ".!?;,"]
    sentence_end_candidates = [position for position in sentence_end_candidates if position >= 0]
    sentence_end = min(sentence_end_candidates, default=len(text))
    before = text[max(sentence_start, start - 80):start]
    after = text[end:min(sentence_end, end + 50)]
    return ACTION_PREFIX.search(before) is not None and SPELL_TARGET_SUFFIX.search(after) is not None


def generate_rules(sql_dir: Path, dbc_dir: Path) -> tuple[dict[str, list[dict]], dict[str, dict], list[dict]]:
    quest_path = sql_dir / "quest_template.sql"
    item_path = sql_dir / "item_template.sql"
    spell_schema = sql_dir / "spell_dbc.sql"
    spell_path = dbc_dir / "Spell.dbc"
    creature_path = sql_dir / "creature_template.sql"
    spawn_path = sql_dir / "creature.sql"
    for path in (quest_path, item_path, creature_path, spawn_path, spell_schema, spell_path):
        if not path.is_file():
            raise FileNotFoundError(f"missing quest spell source: {path}")

    spell_names = spell_names_from_dbc(spell_path, spell_schema)
    # Prefer the longest names first to avoid matching a base spell inside a
    # longer unrelated ability name.
    spell_terms_by_first: dict[str, list[str]] = defaultdict(list)
    spell_word_sets: dict[str, frozenset[str]] = {}
    spell_patterns: dict[str, re.Pattern[str]] = {}
    for term in spell_names:
        words = re.findall(r"[a-z0-9]+", term)
        # Single generic names such as "Bane", "Totem", or "Damage" occur
        # in ordinary prose and produce unsafe matches even with boundaries.
        if len(words) >= 2:
            spell_terms_by_first[words[0]].append(term)
            spell_word_sets[term] = frozenset(words)
    for terms in spell_terms_by_first.values():
        terms.sort(key=lambda name: (-len(name), name))
    quest_columns = [
        "ID", "LogTitle", "LogDescription", "QuestDescription", "QuestCompletionLog",
        "ObjectiveText1", "ObjectiveText2", "ObjectiveText3", "ObjectiveText4",
        "StartItem", "RequiredItemId1", "RequiredItemId2", "RequiredItemId3",
        "RequiredItemId4", "RequiredItemId5", "RequiredItemId6",
        "RequiredNpcOrGo1", "RequiredNpcOrGo2", "RequiredNpcOrGo3", "RequiredNpcOrGo4",
    ]
    qi = _BUILDER.indexes(quest_path, quest_columns)
    item_columns = ["entry", "name", "spellid_1", "spelltrigger_1"]
    ii = _BUILDER.indexes(item_path, item_columns)
    item_uses: dict[int, dict] = {}
    for item_row in _BUILDER.rows(item_path):
        item_id = _BUILDER.as_int(item_row[ii["entry"]])
        spell = _BUILDER.as_int(item_row[ii["spellid_1"]])
        if item_id and spell and _BUILDER.as_int(item_row[ii["spelltrigger_1"]]) == 0:
            item_uses[item_id] = {"name": item_row[ii["name"]] or "", "spell": spell}
    ci = _BUILDER.indexes(creature_path, ["entry", "name"])
    creature_names = {
        _BUILDER.as_int(creature_row[ci["entry"]]): creature_row[ci["name"]] or ""
        for creature_row in _BUILDER.rows(creature_path)
        if _BUILDER.as_int(creature_row[ci["entry"]])
    }
    si = _BUILDER.indexes(spawn_path, ["id1", "id2", "id3"])
    spawned_creatures = {
        _BUILDER.as_int(spawn_row[si[column]])
        for spawn_row in _BUILDER.rows(spawn_path)
        for column in ("id1", "id2", "id3")
        if _BUILDER.as_int(spawn_row[si[column]])
    }
    rules: dict[str, list[dict]] = {}
    item_rules: dict[str, dict] = {}
    item_candidate_quests: set[int] = set()
    unresolved: list[dict] = []

    for row in _BUILDER.rows(quest_path):
        quest_id = _BUILDER.as_int(row[qi["ID"]])
        if not quest_id:
            continue
        text_parts = [row[qi[name]] or "" for name in quest_columns[1:9]]
        text = " ".join(text_parts)
        if not text.strip():
            continue
        targets = []
        for slot in range(1, 5):
            encoded = _BUILDER.as_int(row[qi[f"RequiredNpcOrGo{slot}"]])
            if encoded:
                targets.append({
                    "kind": "gameobject" if encoded < 0 else "creature",
                    "entry": abs(encoded),
                })
        # Quest-bound usable items can be derived without script-specific code:
        # the quest grants/requires the item, item_template maps it to its use
        # spell, and quest text explicitly says to use that item on a target.
        # Keep only a single matching item and target; ambiguous quests go to
        # the unresolved report rather than becoming unsafe runtime behavior.
        eligible_item_ids = {
            _BUILDER.as_int(row[qi[name]])
            for name in ["StartItem", *(f"RequiredItemId{slot}" for slot in range(1, 7))]
        }
        eligible_item_ids.discard(0)
        text_folded = text.casefold()
        named_item_matches = []
        for item_id in eligible_item_ids:
            item = item_uses.get(item_id)
            item_name = item["name"].strip() if item else ""
            if not item_name:
                continue
            pattern = re.compile(r"(?<![\w])" + re.escape(item_name.casefold()) + r"(?![\w])")
            for match in pattern.finditer(text_folded):
                before = text_folded[max(0, match.start() - 40):match.start()]
                after = text_folded[match.end():match.end() + 60]
                if (re.search(r"\b(use|apply|administer)\b", before)
                        and ITEM_TARGET_SUFFIX.search(after)):
                    named_item_matches.append((item_id, item, item_name, match.end()))
                    break
        if named_item_matches:
            item_candidate_quests.add(quest_id)
            item_end = named_item_matches[0][3]
            target_clause = text_folded[item_end:]
            target_clause = re.split(r"[.!?;,]", target_clause, maxsplit=1)[0]

            def name_mentioned(name: str) -> bool:
                folded = name.strip().casefold()
                if not folded:
                    return False
                plural = folded[:-1] + "ies" if folded.endswith("y") else folded + "s"
                return any(re.search(r"(?<![\w])" + re.escape(variant) + r"(?![\w])", target_clause) for variant in (folded, plural))

            grounded_targets = []
            if len(targets) == 1 and targets[0]["kind"] == "creature":
                objective_entry = targets[0]["entry"]
                if objective_entry in spawned_creatures:
                    grounded_targets.append((objective_entry, objective_entry))
                else:
                    # Some scripted objectives credit a transformed entry.
                    # Link it to one spawned creature named in quest text when
                    # the creature name shares specific words with the credit
                    # template. This avoids selecting unrelated quest givers.
                    credit_words = set(re.findall(r"[a-z0-9]+", creature_names.get(objective_entry, "").casefold()))
                    matches = []
                    for candidate in spawned_creatures:
                        candidate_name = creature_names.get(candidate, "").strip()
                        candidate_words = set(re.findall(r"[a-z0-9]+", candidate_name.casefold()))
                        if (len(candidate_words) > 1 and candidate_words & credit_words
                                and name_mentioned(candidate_name)):
                            matches.append(candidate)
                    if len(matches) == 1:
                        grounded_targets.append((objective_entry, matches[0]))
            elif len(targets) > 1:
                for target in targets:
                    if target["kind"] == "creature" and target["entry"] in spawned_creatures:
                        name = creature_names.get(target["entry"], "")
                        if name_mentioned(name):
                            grounded_targets.append((target["entry"], target["entry"]))

            grounded_targets = sorted(set(grounded_targets))
            if len(named_item_matches) == 1 and len(grounded_targets) == 1:
                item_id, item, item_name, _ = named_item_matches[0]
                objective_entry, target_entry = grounded_targets[0]
                item_rules[str(quest_id)] = {
                    "kind": "creature", "entry": target_entry,
                    "objective_entry": objective_entry,
                    "item": item_id, "spell": item["spell"], "count": 1,
                    "name": item_name, "source": "quest_text_quest_item_and_item_template",
                }
            else:
                unresolved.append({"quest": quest_id, "title": row[qi["LogTitle"]] or "", "reason": "ambiguous quest item use", "items": [item_id for item_id, _, _, _ in named_item_matches]})
        if not targets:
            continue

        found: dict[str, set[int]] = defaultdict(set)
        folded_text = text.casefold()
        words_in_text = frozenset(re.findall(r"[a-z0-9]+", folded_text))
        candidate_terms = {
            term
            for word in words_in_text
            for term in spell_terms_by_first.get(word, ())
            if spell_word_sets[term].issubset(words_in_text)
        }
        matches: list[tuple[int, int, str]] = []
        for term in candidate_terms:
            pattern = spell_patterns.get(term)
            if pattern is None:
                pattern = re.compile(r"(?<![\w])" + re.escape(term) + r"(?![\w])", re.IGNORECASE)
                spell_patterns[term] = pattern
            match = pattern.search(folded_text)
            if match and _near_action(folded_text, match.start(), match.end()):
                matches.append((match.start(), match.end(), term))
        matches.sort(key=lambda match: (match[0], -(match[1] - match[0])))
        for start, end, term in matches:
            if any(other_start <= start and end <= other_end and (other_start, other_end) != (start, end)
                   for other_start, other_end, _ in matches):
                continue
            found[term].update(spell_names[term][1])

        # Do not feed an item activation into the player-spell cast path. If
        # target grounding was ambiguous, keep it out of runtime rules and
        # report it for review instead.
        if not found or quest_id in item_candidate_quests:
            continue
        if len(targets) != 1:
            unresolved.append({"quest": quest_id, "title": row[qi["LogTitle"]] or "", "reason": "multiple quest targets", "spell_names": sorted(found)})
            continue
        target = targets[0]
        rules[str(quest_id)] = [
            {
                "kind": target["kind"],
                "entry": target["entry"],
                "name": spell_names[term][0],
                "spells": sorted(spell_ids),
                "source": "quest_text_and_spell_dbc",
            }
            for term, spell_ids in sorted(found.items())
        ]

    return rules, item_rules, unresolved


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    infra_catalog = repo / "crates/wow-infra/data/world-knowledge/azerothcore-catalog.json"
    legacy_catalog = repo / "crates/wow-policy/data/azerothcore-quest-hints.json"
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("azerothcore_root", type=Path)
    parser.add_argument("--dbc-dir", type=Path, required=True)
    parser.add_argument(
        "--quest-hints",
        type=Path,
        default=infra_catalog if infra_catalog.exists() else legacy_catalog,
        help="existing generated quest catalog to extend (read-only input)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="output catalog path; defaults to replacing --quest-hints",
    )
    parser.add_argument("--verbose-unresolved", action="store_true", help="print each ungrounded quest candidate")
    args = parser.parse_args()

    rules, item_rules, unresolved = generate_rules(args.azerothcore_root / "data/sql/base/db_world", args.dbc_dir)
    document = json.loads(args.quest_hints.read_text(encoding="utf-8"))
    document["format_version"] = max(int(document.get("format_version", 0)), 5)
    document["quest_spell_rules"] = rules
    document["quest_item_use_rules"] = item_rules
    document["unresolved_quest_action_hints"] = unresolved
    document.setdefault("source", {})["quest_spell_rule_generator"] = "tools/generate_quest_spell_hints.py"
    sql_dir = args.azerothcore_root / "data/sql/base/db_world"
    document["source"]["quest_spell_inputs"] = {
        path.name: hashlib.sha256(path.read_bytes()).hexdigest()
        for path in (
            sql_dir / "quest_template.sql", sql_dir / "item_template.sql",
            sql_dir / "creature_template.sql", sql_dir / "creature.sql",
            sql_dir / "spell_dbc.sql", args.dbc_dir / "Spell.dbc",
        )
    }
    output = args.output or args.quest_hints
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_suffix(output.suffix + ".tmp")
    temporary.write_text(json.dumps(document, separators=(",", ":"), ensure_ascii=False), encoding="utf-8")
    temporary.replace(output)
    print(f"wrote {len(rules)} quest spell rules to {output}")
    print(f"generated {len(item_rules)} quest-bound item-use rules")
    print(f"could not ground {len(unresolved)} matched quest rules")
    if args.verbose_unresolved:
        for item in unresolved:
            details = item.get("spell_names", item.get("items", []))
            print(f"unresolved quest {item['quest']}: {item['title']} [{item['reason']}] ({', '.join(map(str, details))})")


if __name__ == "__main__":
    main()
