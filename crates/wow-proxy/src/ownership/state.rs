use wow_domain::{MovementEpoch, OwnershipGeneration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMode {
    Bot,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    Bot,
    Player,
}

impl ControlMode {
    pub fn owns(self, source: ClientKind) -> bool {
        matches!(
            (self, source),
            (Self::Bot, ClientKind::Bot) | (Self::Manual, ClientKind::Player)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    Bot,
    TakingManual,
    Manual,
    TakingBot,
    ReconnectingBot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountAttendance {
    Unattended,
    PlayerPresent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttendedControlState {
    Unattended,
    PlayerAttachedManualOff,
    PlayerAttachedBotOn,
    PlayerAttachedBotOnUserMoving,
    ReclaimingAfterPlayerLogout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionTicket {
    pub generation: OwnershipGeneration,
    pub target: ControlMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnershipSnapshot {
    pub mode: ControlMode,
    pub requested: ControlMode,
    pub phase: SessionPhase,
    pub attendance: AccountAttendance,
    pub attended_control: AttendedControlState,
    pub generation: OwnershipGeneration,
    pub movement_epoch: MovementEpoch,
}

impl OwnershipSnapshot {
    pub fn transitioning(self) -> bool {
        matches!(
            self.phase,
            SessionPhase::TakingManual | SessionPhase::TakingBot | SessionPhase::ReconnectingBot
        )
    }
    pub fn ticket(self) -> TransitionTicket {
        TransitionTicket {
            generation: self.generation,
            target: self.requested,
        }
    }
    pub fn permits(self, source: ClientKind) -> bool {
        if self.transitioning() {
            return false;
        }
        match self.attended_control {
            AttendedControlState::PlayerAttachedManualOff
            | AttendedControlState::PlayerAttachedBotOnUserMoving => source == ClientKind::Player,
            AttendedControlState::PlayerAttachedBotOn => source == ClientKind::Bot,
            AttendedControlState::ReclaimingAfterPlayerLogout => false,
            AttendedControlState::Unattended => self.mode.owns(source),
        }
    }
    pub fn player_present(self) -> bool {
        self.attendance == AccountAttendance::PlayerPresent
    }
    pub fn bot_allowed(self) -> bool {
        matches!(
            self.attended_control,
            AttendedControlState::PlayerAttachedBotOn
        ) || (self.attended_control == AttendedControlState::Unattended
            && self.mode == ControlMode::Bot)
    }
}

pub struct AccountOwnership {
    state: OwnershipSnapshot,
}

impl AccountOwnership {
    pub fn new(mode: ControlMode) -> Self {
        Self {
            state: OwnershipSnapshot {
                mode,
                requested: mode,
                phase: if mode == ControlMode::Bot {
                    SessionPhase::Bot
                } else {
                    SessionPhase::Manual
                },
                attendance: AccountAttendance::Unattended,
                attended_control: AttendedControlState::Unattended,
                generation: OwnershipGeneration::ZERO,
                movement_epoch: MovementEpoch(1),
            },
        }
    }
    pub fn snapshot(&self) -> OwnershipSnapshot {
        self.state
    }
    pub fn fence_movement(&mut self) {
        self.state.movement_epoch = self.state.movement_epoch.next();
    }
    fn advance_generation(&mut self) {
        self.state.generation = self.state.generation.next();
        self.fence_movement();
    }

    pub fn player_attached_manual_off(&mut self) {
        self.advance_generation();
        self.state.mode = ControlMode::Manual;
        self.state.requested = ControlMode::Manual;
        self.state.phase = SessionPhase::Manual;
        self.state.attendance = AccountAttendance::PlayerPresent;
        self.state.attended_control = AttendedControlState::PlayerAttachedManualOff;
    }
    pub fn player_attached_bot_on(&mut self) {
        self.advance_generation();
        self.state.mode = ControlMode::Bot;
        self.state.requested = ControlMode::Bot;
        self.state.phase = SessionPhase::Bot;
        self.state.attendance = AccountAttendance::PlayerPresent;
        self.state.attended_control = AttendedControlState::PlayerAttachedBotOn;
    }
    pub fn player_logout_reclaiming(&mut self, request_reclaim: bool) {
        self.advance_generation();
        self.state.mode = ControlMode::Manual;
        self.state.requested = if request_reclaim {
            ControlMode::Bot
        } else {
            ControlMode::Manual
        };
        self.state.phase = SessionPhase::Manual;
        self.state.attendance = AccountAttendance::PlayerPresent;
        self.state.attended_control = AttendedControlState::ReclaimingAfterPlayerLogout;
    }
    pub fn player_logout_no_bot(&mut self) {
        self.advance_generation();
        self.state.mode = ControlMode::Manual;
        self.state.requested = ControlMode::Manual;
        self.state.phase = SessionPhase::Manual;
        self.state.attendance = AccountAttendance::Unattended;
        self.state.attended_control = AttendedControlState::Unattended;
    }
    pub fn player_movement_takeover(&mut self) {
        self.advance_generation();
        self.state.mode = ControlMode::Manual;
        self.state.requested = ControlMode::Bot;
        self.state.phase = SessionPhase::Manual;
        self.state.attendance = AccountAttendance::PlayerPresent;
        self.state.attended_control = AttendedControlState::PlayerAttachedBotOnUserMoving;
    }
    pub fn begin(&mut self, target: ControlMode) -> TransitionTicket {
        self.advance_generation();
        self.state.requested = target;
        self.state.phase = if target == ControlMode::Manual {
            SessionPhase::TakingManual
        } else {
            SessionPhase::TakingBot
        };
        if self.state.attendance == AccountAttendance::PlayerPresent
            && self.state.attended_control != AttendedControlState::ReclaimingAfterPlayerLogout
        {
            self.state.attended_control = if target == ControlMode::Bot {
                AttendedControlState::PlayerAttachedBotOn
            } else {
                AttendedControlState::PlayerAttachedManualOff
            };
        }
        self.state.ticket()
    }
    pub fn reconnect(&mut self) -> TransitionTicket {
        let t = self.begin(ControlMode::Bot);
        self.state.phase = SessionPhase::ReconnectingBot;
        t
    }
    pub fn is_current(&self, ticket: TransitionTicket) -> bool {
        self.state.transitioning() && self.state.ticket() == ticket
    }
    pub fn commit(&mut self, ticket: TransitionTicket) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.state.mode = ticket.target;
        self.state.phase = if ticket.target == ControlMode::Bot {
            SessionPhase::Bot
        } else {
            SessionPhase::Manual
        };
        if self.state.attended_control == AttendedControlState::ReclaimingAfterPlayerLogout {
            self.state.attendance = AccountAttendance::Unattended;
            self.state.attended_control = AttendedControlState::Unattended;
        } else if self.state.attendance == AccountAttendance::PlayerPresent {
            self.state.attended_control = if ticket.target == ControlMode::Bot {
                AttendedControlState::PlayerAttachedBotOn
            } else {
                AttendedControlState::PlayerAttachedManualOff
            };
        }
        true
    }
    pub fn fail(&mut self, ticket: TransitionTicket) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.force_manual();
        true
    }
    pub fn force_manual(&mut self) {
        let t = self.begin(ControlMode::Manual);
        let _ = self.commit(t);
    }
}

impl Default for AccountOwnership {
    fn default() -> Self {
        Self::new(ControlMode::Bot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transition_fences_both_sources_and_stale_ticket_cannot_commit() {
        let mut o = AccountOwnership::default();
        let a = o.begin(ControlMode::Manual);
        assert!(!o.snapshot().permits(ClientKind::Bot));
        assert!(!o.snapshot().permits(ClientKind::Player));
        let b = o.begin(ControlMode::Manual);
        assert_ne!(a.generation, b.generation);
        assert!(!o.commit(a));
        assert!(o.commit(b));
        assert!(o.snapshot().permits(ClientKind::Player));
    }
}
