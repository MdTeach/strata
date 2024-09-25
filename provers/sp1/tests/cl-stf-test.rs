#[cfg(feature = "prover")]

mod test {
    use alpen_express_state::{block::L2Block, chain_state::ChainState};
    use express_sp1_adapter::{SP1Host, SP1ProofInputBuilder, SP1Verifier};
    use express_sp1_guest_builder::GUEST_CL_STF_ELF;
    use express_zkvm::{ZKVMHost, ZKVMInputBuilder, ZKVMVerifier};

    fn get_prover_input() -> (ChainState, L2Block) {
        let prev_state_data: &[u8] =
            include_bytes!("../../test-util/cl_stfs/slot-1/prev_chstate.borsh");
        let new_block_data: &[u8] =
            include_bytes!("../../test-util/cl_stfs/slot-1/final_block.borsh");

        let prev_state: ChainState = borsh::from_slice(prev_state_data).unwrap();
        let block: L2Block = borsh::from_slice(new_block_data).unwrap();

        (prev_state, block)
    }

    #[test]
    fn test_reth_stf_guest_code_trace_generation() {
        // let input: ELProofInput = bincode::deserialize(ENCODED_PROVER_INPUT).unwrap();
        let input = get_prover_input();
        let input_ser = borsh::to_vec(&input).unwrap();

        let prover = SP1Host::init(GUEST_CL_STF_ELF.into(), Default::default());

        // TODO: handle this properly
        let prover_input = SP1ProofInputBuilder::new()
            .write(&input_ser)
            .unwrap()
            .build()
            .unwrap();

        let (proof, _) = prover
            .prove(prover_input)
            .expect("Failed to generate proof");

        let new_state_ser = SP1Verifier::extract_public_output::<Vec<u8>>(&proof)
            .expect("Failed to extract public outputs");

        let _new_state: ChainState = borsh::from_slice(&new_state_ser).unwrap();
    }
}