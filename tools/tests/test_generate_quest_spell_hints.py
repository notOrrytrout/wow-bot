import importlib.util
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "generate_quest_spell_hints.py"
SPEC = importlib.util.spec_from_file_location("generate_quest_spell_hints", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


class GroundedItemTargetTests(unittest.TestCase):
    def test_named_temporary_creature_is_grounded_without_static_spawn(self):
        targets = [{"kind": "creature", "entry": 21731}]

        result = GENERATOR.grounded_item_targets(
            targets,
            {21731: "Electromental"},
            set(),
            "on the resultant electromental to encase it",
        )

        self.assertEqual(result, [(21731, 21731)])

    def test_single_quest_target_allows_scripted_variant_name(self):
        targets = [{"kind": "creature", "entry": 21731}]

        result = GENERATOR.grounded_item_targets(
            targets,
            {21731: "Encased Electromental", 21729: "Electromental"},
            set(),
            "on the resultant electromental to encase it",
        )

        self.assertEqual(result, [(21731, 21729)])

    def test_temporary_creature_requires_name_in_item_use_clause(self):
        targets = [{"kind": "creature", "entry": 21731}]

        result = GENERATOR.grounded_item_targets(
            targets,
            {21731: "Electromental"},
            set(),
            "on the nearby ogre",
        )

        self.assertEqual(result, [])

    def test_multi_target_objective_selects_only_the_named_creature(self):
        targets = [
            {"kind": "creature", "entry": 21731},
            {"kind": "creature", "entry": 21732},
        ]

        result = GENERATOR.grounded_item_targets(
            targets,
            {21731: "Electromental", 21732: "Bladespire Ogre"},
            set(),
            "on the electromentals",
        )

        self.assertEqual(result, [(21731, 21731)])

    def test_multiple_named_objectives_remain_ambiguous(self):
        targets = [
            {"kind": "creature", "entry": 21731},
            {"kind": "creature", "entry": 21732},
        ]

        result = GENERATOR.grounded_item_targets(
            targets,
            {21731: "Electromental", 21732: "Electromental"},
            set(),
            "on the electromentals",
        )

        self.assertEqual(len(result), 2)

    def test_static_spawn_breaks_a_duplicate_template_name_tie(self):
        targets = [{"kind": "creature", "entry": 31364}]

        result = GENERATOR.grounded_item_targets(
            targets,
            {31137: "Frostbrood Skytalon", 31583: "Frostbrood Skytalon", 31364: "Frostbrood Skytalon KC Bunny"},
            {31137},
            "at the Broken Front to attract 3 Frostbrood Skytalons",
        )

        self.assertEqual(result, [(31364, 31137)])

    def test_specific_live_target_name_wins_over_generic_name(self):
        targets = [{"kind": "creature", "entry": 12299}]

        result = GENERATOR.grounded_item_targets(
            targets,
            {883: "Deer", 12298: "Sickly Deer", 12299: "Cured Deer"},
            set(),
            "on 10 sickly deer",
        )

        self.assertEqual(result, [(12299, 12298)])


if __name__ == "__main__":
    unittest.main()
