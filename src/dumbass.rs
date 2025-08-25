use ra_ap_ide::AnalysisHost;

pub enum Dumbass {
    // Initializes the codebase and feeds information into the children
    Head(),
    // Expert of a region of a codebase, not sure how to shard it yet, the context will
    // be persistent and updated as code gets updated
    Expert(),
    // Shouldn't have persistent context, the only job is to make changes then is gone
    Worker(),
}
pub struct Agen {
    pub host: AnalysisHost,
}
