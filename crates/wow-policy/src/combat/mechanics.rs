#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mechanic {
    Interrupt,
    Dispel,
    MoveOut,
    StopCast,
    Defensive,
}
