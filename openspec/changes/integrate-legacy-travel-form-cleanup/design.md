# Design

The lane tracks one pending on-foot transition. It sends typed mount-cancel and observed-aura-cancel commands at one-second intervals. It resumes the blocked command only after authoritative mounted state is false and the known travel-form auras are absent. The transition stops after five seconds or five attempts, and it never sends the blocked command without confirmation.

Movement arrival starts the same cleanup state when the server still reports a mount or either supported travel form. If a resumed action needs an on-foot player, that action joins the cleanup state and waits for confirmation. Pauses, mission changes, and death recovery cancel the pending voluntary action. Existing final action validation still checks the resumed command against current state.

`CancelMount` and `CancelAura` are typed gameplay commands. The normal command validator and transport path authorize and encode them. This change does not add raw packet access to the lane.
