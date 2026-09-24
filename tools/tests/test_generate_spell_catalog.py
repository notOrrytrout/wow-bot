import importlib.util
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "generate_spell_catalog.py"
SPEC = importlib.util.spec_from_file_location("generate_spell_catalog", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


class AttackSpellClassificationTests(unittest.TestCase):
    def row_with_effects(self, effects):
        row = [0] * GENERATOR.SPELL_FIELDS
        for index, (effect, aura) in enumerate(effects):
            row[GENERATOR.FIELDS["effect_start"] + index] = effect
            row[GENERATOR.FIELDS["effect_aura_start"] + index] = aura
        return tuple(row)

    def test_direct_damage_is_offensive(self):
        row = self.row_with_effects([(1, 0), (0, 0), (0, 0)])

        self.assertEqual(GENERATOR.attack_spell_flags(row), (True, False))

    def test_periodic_damage_and_leech_are_offensive_damage_over_time(self):
        for aura in (GENERATOR.SPELL_AURA_PERIODIC_DAMAGE, GENERATOR.SPELL_AURA_PERIODIC_LEECH):
            with self.subTest(aura=aura):
                row = self.row_with_effects(
                    [(GENERATOR.SPELL_EFFECT_APPLY_AURA, aura), (0, 0), (0, 0)]
                )

                self.assertEqual(GENERATOR.attack_spell_flags(row), (True, True))

    def test_non_damage_aura_is_not_offensive(self):
        row = self.row_with_effects([(GENERATOR.SPELL_EFFECT_APPLY_AURA, 4), (0, 0), (0, 0)])

        self.assertEqual(GENERATOR.attack_spell_flags(row), (False, False))

    def test_pet_lifecycle_effect_cannot_become_offensive(self):
        row = self.row_with_effects(
            [(1, 0), (min(GENERATOR.PET_LIFECYCLE_EFFECTS), 0), (0, 0)]
        )

        self.assertEqual(GENERATOR.attack_spell_flags(row), (False, False))


if __name__ == "__main__":
    unittest.main()
