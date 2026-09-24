use wow_domain::WorkerGeneration;
#[derive(Clone, Copy, Debug)]
pub struct WorkerAuthority {
    pub generation: WorkerGeneration,
}
