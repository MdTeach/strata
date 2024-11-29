use anyhow::Context;
use strata_proofimpl_cl_agg::{ClAggInput, ClAggProver};
use strata_proofimpl_cl_stf::L2BatchProofOutput;
#[cfg(feature = "risc0")]
use strata_risc0_adapter::{Risc0Host, Risc0ProofInputBuilder};
#[cfg(feature = "risc0")]
use strata_risc0_guest_builder::{GUEST_RISC0_CL_AGG_ELF, GUEST_RISC0_CL_AGG_ID};
#[cfg(feature = "sp1")]
use strata_sp1_adapter::{SP1Host, SP1ProofInputBuilder};
#[cfg(feature = "sp1")]
use strata_sp1_guest_builder::{GUEST_CL_AGG_PK, GUEST_CL_AGG_VK, GUEST_CL_AGG_VK_HASH_STR};
use strata_zkvm::{
    AggregationInput, Proof, ProofType, VerificationKey, ZkVmHost, ZkVmInputBuilder, ZkVmProver,
    ZkVmResult,
};

use crate::{cl::ClProofGenerator, proof_generator::ProofGenerator};

pub struct L2BatchProofGenerator {
    cl_proof_generator: ClProofGenerator,
}

impl L2BatchProofGenerator {
    pub fn new(cl_proof_generator: ClProofGenerator) -> Self {
        Self { cl_proof_generator }
    }
}

impl ProofGenerator<(u64, u64), ClAggProver> for L2BatchProofGenerator {
    fn get_input(&self, heights: &(u64, u64)) -> ZkVmResult<ClAggInput> {
        let (start_height, end_height) = *heights;
        let mut batch = Vec::new();

        for block_num in start_height..=end_height {
            let cl_proof = self.cl_proof_generator.get_proof(&block_num)?;
            batch.push(cl_proof);
        }

        let cl_stf_vk = self.cl_proof_generator.get_host().get_verification_key();
        Ok(ClAggInput { batch, cl_stf_vk })
    }

    fn gen_proof(&self, heights: &(u64, u64)) -> ZkVmResult<(Proof, L2BatchProofOutput)> {
        let input = self.get_input(heights)?;
        let host = self.get_host();
        ClAggProver::prove(&input, &host)
    }

    fn get_proof_id(&self, heights: &(u64, u64)) -> String {
        let (start_height, end_height) = *heights;
        format!("l2_batch_{}_{}", start_height, end_height)
    }

    fn get_host(&self) -> impl ZkVmHost {
        #[cfg(feature = "risc0")]
        {
            // If both features are enabled, prioritize 'risc0'
            Risc0Host::init(GUEST_RISC0_CL_AGG_ELF)
        }

        #[cfg(all(feature = "sp1", not(feature = "risc0")))]
        {
            // Only use 'sp1' if 'risc0' is not enabled
            return SP1Host::new_from_bytes(&GUEST_CL_AGG_PK, &GUEST_CL_AGG_VK);
        }
    }

    fn get_short_program_id(&self) -> String {
        #[cfg(feature = "risc0")]
        {
            // If both features are enabled, prioritize 'risc0'
            hex::encode(GUEST_RISC0_CL_AGG_ID[0].to_le_bytes())
        }
        #[cfg(all(feature = "sp1", not(feature = "risc0")))]
        {
            // Only use 'sp1' if 'risc0' is not enabled
            GUEST_CL_AGG_VK_HASH_STR.to_string().split_off(58)
        }
    }
}

// Run test if any of sp1 or risc0 feature is enabled and the test is being run in release mode
#[cfg(test)]
#[cfg(all(any(feature = "sp1", feature = "risc0"), not(debug_assertions)))]
mod test {
    use crate::{ClProofGenerator, ElProofGenerator, L2BatchProofGenerator, ProofGenerator};

    #[test]
    fn test_cl_agg_guest_code_trace_generation() {
        let el_prover = ElProofGenerator::new();
        let cl_prover = ClProofGenerator::new(el_prover);
        let cl_agg_prover = L2BatchProofGenerator::new(cl_prover);

        let _ = cl_agg_prover.get_proof(&(1, 3)).unwrap();
    }
}