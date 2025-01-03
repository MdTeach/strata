/// A proof report containing a performance stats about proof generation.
#[derive(Debug, Clone)]
pub struct ProofReport {
    pub cycles: u64,
    pub report_name: String,
}
