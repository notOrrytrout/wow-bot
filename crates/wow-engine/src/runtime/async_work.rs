use wow_domain::ValidityStamp;
#[derive(Clone, Debug)]
pub struct Stamped<T> {
    pub stamp: ValidityStamp,
    pub value: T,
}
