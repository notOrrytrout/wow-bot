# Design

The engine will retain its 250 ms mission-planning tick and use a separate 100 ms tick only while a movement operation is active. The shared navigation controller will use a 0.7 yard maximum step. This pairs a smaller step with a faster update while keeping ground movement near its current maximum pace.

The configured player bridge will place each encoded bot movement frame in a latest-value watch channel after the upstream write succeeds. The bridge will send mirror frames from a separate select branch. A monotonically increasing sequence detects coalesced updates; when a gap occurs, the bridge sends the latest absolute position and logs the recovery. A local mirror epoch invalidates queued frames after explicit player movement, `.bot on`, `.bot off`, or a mission change. The receiver discards frames from older epochs and duplicate sequences.

Movement encoding will reject non-finite current positions and orientations. A downstream mirror write failure will include account and sequence context, log the reason, and close the world bridge so the existing reconnect path can rebuild it safely.

The latest-value channel bounds queued movement data and recovers from missed intermediate poses. It does not change the navigation route or invent a path between route steps.
