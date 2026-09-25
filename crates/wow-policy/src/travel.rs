use wow_infra::config::runtime_data::RuntimeTuning;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceSafety {
    Unknown,
    Unsafe,
    Safe,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TravelAbilityKind {
    SpeedForm,
    Mount,
}

/// A travel ability that the caller confirmed as known and ready from the current snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadyTravelAbility {
    kind: TravelAbilityKind,
    spell: u32,
}

/// Check authoritative known-spell and resource state before the lane passes an
/// ability to the shared travel eligibility policy.
pub fn ready_travel_ability(
    snapshot: &wow_state::Snapshot,
    kind: TravelAbilityKind,
    spell: u32,
    now_ms: u64,
) -> Option<ReadyTravelAbility> {
    crate::combat::readiness::check_spell_readiness(snapshot, spell, None, now_ms).ok()?;
    Some(ReadyTravelAbility { kind, spell })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TravelAction {
    kind: TravelAbilityKind,
    spell: u32,
}

impl TravelAction {
    pub fn kind(self) -> TravelAbilityKind {
        self.kind
    }

    pub fn spell(self) -> u32 {
        self.spell
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TravelContext {
    pub alive: Option<bool>,
    pub in_combat: Option<bool>,
    pub controlled: bool,
    pub mounted: bool,
    pub surface: SurfaceSafety,
    pub distance_yards: f32,
    pub ready_ability: Option<ReadyTravelAbility>,
}

/// Select a travel ability only when all safety facts are authoritative and its
/// configured distance threshold is met. The caller must derive `ready_ability`
/// with the shared spell-readiness policy against the current snapshot.
pub fn select_travel_action(
    context: TravelContext,
    tuning: &RuntimeTuning,
) -> Option<TravelAction> {
    if context.alive != Some(true)
        || context.in_combat != Some(false)
        || context.controlled
        || context.mounted
        || context.surface != SurfaceSafety::Safe
        || !context.distance_yards.is_finite()
        || context.distance_yards < 0.0
    {
        return None;
    }

    let ability = context.ready_ability?;
    let minimum_distance = match ability.kind {
        TravelAbilityKind::SpeedForm => tuning.movement.travel_speed_form_min_yards,
        TravelAbilityKind::Mount if tuning.maintenance.auto_mount_enabled => {
            tuning.maintenance.mount_min_travel_yards
        }
        TravelAbilityKind::Mount => return None,
    } as f32;
    (context.distance_yards >= minimum_distance).then_some(TravelAction {
        kind: ability.kind,
        spell: ability.spell,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::EntityId;
    use wow_state::{
        AuthoritativeState, Snapshot,
        entities::{EntityKind, EntityState},
    };

    fn safe_context(
        distance_yards: f32,
        ready_ability: Option<ReadyTravelAbility>,
    ) -> TravelContext {
        TravelContext {
            alive: Some(true),
            in_combat: Some(false),
            controlled: false,
            mounted: false,
            surface: SurfaceSafety::Safe,
            distance_yards,
            ready_ability,
        }
    }

    fn ready_form() -> ReadyTravelAbility {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(1);
        state.capabilities.class_id = Some(11);
        state.capabilities.spells.insert(783);
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                power_type: Some(0),
                power: Some((100, 100)),
                health: Some((100, 100)),
                base_health: Some(100),
                base_mana: Some(100),
                power_cost_modifiers: Some([0; 7]),
                power_cost_multipliers: Some([0.0; 7]),
                shapeshift_form: Some(0),
                aura_state: Some(0),
                ..Default::default()
            },
        );
        ready_travel_ability(
            &Snapshot::from_state(&state),
            TravelAbilityKind::SpeedForm,
            783,
            u64::MAX,
        )
        .expect("known Travel Form has sufficient resources")
    }

    fn mount() -> ReadyTravelAbility {
        ReadyTravelAbility {
            kind: TravelAbilityKind::Mount,
            spell: 458,
        }
    }

    #[test]
    fn selects_a_ready_speed_form_on_a_safe_surface() {
        let form = ready_form();
        let action =
            select_travel_action(safe_context(30.0, Some(form)), &RuntimeTuning::default());
        assert_eq!(
            action,
            Some(TravelAction {
                kind: TravelAbilityKind::SpeedForm,
                spell: 783,
            })
        );
    }

    #[test]
    fn readiness_check_rejects_a_spell_that_is_not_known() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(1);
        state.capabilities.class_id = Some(11);
        assert_eq!(
            ready_travel_ability(
                &Snapshot::from_state(&state),
                TravelAbilityKind::SpeedForm,
                783,
                u64::MAX,
            ),
            None
        );
    }

    #[test]
    fn speed_form_threshold_is_inclusive_and_configurable() {
        let form = ready_form();
        let tuning = RuntimeTuning {
            movement: wow_infra::config::runtime_data::MovementTuning {
                travel_speed_form_min_yards: 35,
            },
            ..RuntimeTuning::default()
        };
        assert_eq!(
            select_travel_action(safe_context(34.99, Some(form)), &tuning),
            None
        );
        assert!(select_travel_action(safe_context(35.0, Some(form)), &tuning).is_some());
    }

    #[test]
    fn mount_requires_enabled_setting_and_mount_threshold() {
        let mount = mount();
        let disabled = RuntimeTuning::default();
        assert_eq!(
            select_travel_action(safe_context(100.0, Some(mount)), &disabled),
            None
        );

        let enabled = RuntimeTuning {
            maintenance: wow_infra::config::runtime_data::MaintenanceTuning {
                auto_mount_enabled: true,
                mount_min_travel_yards: 80,
            },
            ..RuntimeTuning::default()
        };
        assert_eq!(
            select_travel_action(safe_context(79.99, Some(mount)), &enabled),
            None
        );
        assert!(select_travel_action(safe_context(80.0, Some(mount)), &enabled).is_some());
    }

    #[test]
    fn unsafe_unknown_or_unavailable_states_fail_closed() {
        let form = ready_form();
        let mut context = safe_context(100.0, Some(form));
        for surface in [SurfaceSafety::Unknown, SurfaceSafety::Unsafe] {
            context.surface = surface;
            assert_eq!(
                select_travel_action(context, &RuntimeTuning::default()),
                None
            );
        }

        context = safe_context(100.0, Some(form));
        context.alive = None;
        assert_eq!(
            select_travel_action(context, &RuntimeTuning::default()),
            None
        );
        context.alive = Some(false);
        assert_eq!(
            select_travel_action(context, &RuntimeTuning::default()),
            None
        );
        context.alive = Some(true);
        context.in_combat = None;
        assert_eq!(
            select_travel_action(context, &RuntimeTuning::default()),
            None
        );
        context.in_combat = Some(true);
        assert_eq!(
            select_travel_action(context, &RuntimeTuning::default()),
            None
        );
        context.in_combat = Some(false);
        context.controlled = true;
        assert_eq!(
            select_travel_action(context, &RuntimeTuning::default()),
            None
        );
        context.controlled = false;
        context.mounted = true;
        assert_eq!(
            select_travel_action(context, &RuntimeTuning::default()),
            None
        );
        context.mounted = false;
        context.ready_ability = None;
        assert_eq!(
            select_travel_action(context, &RuntimeTuning::default()),
            None
        );
    }
}
