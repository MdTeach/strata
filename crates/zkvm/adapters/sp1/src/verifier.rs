use anyhow::{Context, Ok, Result};
use express_zkvm::{Proof, VerificationKey, ZKVMVerifier};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::to_vec;
use snark_bn254_verifier::Groth16Verifier;
use sp1_sdk::{ProverClient, SP1ProofWithPublicValues, SP1PublicValues, SP1VerifyingKey};
use substrate_bn::Fr;

/// A verifier for the `SP1` zkVM, responsible for verifying proofs generated by the host
pub struct SP1Verifier;

// Copied from ~/.sp1/circuits/v2.0.0/groth16_vk.bin
// This is same for all the SP1 programs that uses v2.0.0
pub const GROTH16_VK_BYTES: &[u8] = include_bytes!("groth16_vk.bin");

impl ZKVMVerifier for SP1Verifier {
    fn verify(verification_key: &VerificationKey, proof: &Proof) -> anyhow::Result<()> {
        let proof: SP1ProofWithPublicValues = bincode::deserialize(proof.as_bytes())?;
        let vkey: SP1VerifyingKey = bincode::deserialize(&verification_key.0)?;

        let client = ProverClient::new();
        client.verify(&proof, &vkey)?;

        Ok(())
    }

    fn verify_with_public_params<T: DeserializeOwned + serde::Serialize>(
        verification_key: &VerificationKey,
        public_params: T,
        proof: &Proof,
    ) -> anyhow::Result<()> {
        let mut proof: SP1ProofWithPublicValues = bincode::deserialize(proof.as_bytes())?;
        let vkey: SP1VerifyingKey = bincode::deserialize(&verification_key.0)?;

        let client = ProverClient::new();
        client.verify(&proof, &vkey)?;

        let actual_public_parameter: T = proof.public_values.read();

        // TODO: use custom ZKVM error
        anyhow::ensure!(
            to_vec(&actual_public_parameter)? == to_vec(&public_params)?,
            "Failed to verify proof given the public param"
        );

        Ok(())
    }

    fn verify_groth16(proof: &[u8], vkey_hash: &[u8], committed_values_raw: &[u8]) -> Result<()> {
        let vk = GROTH16_VK_BYTES;

        // Convert vkey_hash to Fr, mapping the error to anyhow::Error
        let vkey_hash_fr = Fr::from_slice(vkey_hash)
            .map_err(|e| anyhow::anyhow!(e))
            .context("Unable to convert vkey_hash to Fr")?;

        let committed_values_digest = SP1PublicValues::from(committed_values_raw)
            .hash_bn254()
            .to_bytes_be();

        // Convert committed_values_digest to Fr, mapping the error to anyhow::Error
        let committed_values_digest_fr = Fr::from_slice(&committed_values_digest)
            .map_err(|e| anyhow::anyhow!(e))
            .context("Unable to convert committed_values_digest to Fr")?;

        // Perform the Groth16 verification, mapping any error to anyhow::Error
        let verification_result =
            Groth16Verifier::verify(proof, vk, &[vkey_hash_fr, committed_values_digest_fr])
                .map_err(|e| anyhow::anyhow!(e))
                .context("Groth16 verification failed")?;

        if verification_result {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Groth16 proof verification returned false"))
        }
    }

    fn extract_public_output<T: Serialize + DeserializeOwned>(proof: &Proof) -> anyhow::Result<T> {
        let mut proof: SP1ProofWithPublicValues = bincode::deserialize(proof.as_bytes())?;
        let public_params: T = proof.public_values.read();
        Ok(public_params)
    }
}

// NOTE: SP1 prover runs in release mode only; therefore run the tests on release mode only
#[cfg(test)]
mod tests {

    use num_bigint::BigUint;
    use num_traits::Num;

    use super::*;

    #[test]
    fn test_groth16_verification() {
        let vk = "0x00b01ae596b4e51843484ff71ccbd0dd1a030af70b255e6b9aad50b81d81266f";

        let raw_groth16_proof = include_bytes!("../tests/proofs/proof-groth16.bin");
        let proof: SP1ProofWithPublicValues =
            bincode::deserialize(raw_groth16_proof).expect("Failed to deserialize Groth16 proof");

        let groth16_proof_bytes = proof
            .proof
            .try_as_groth_16()
            .expect("Failed to convert proof to Groth16")
            .raw_proof;
        let groth16_proof =
            hex::decode(&groth16_proof_bytes).expect("Failed to decode Groth16 proof");

        let vkey_hash = BigUint::from_str_radix(
            vk.strip_prefix("0x").expect("vkey should start with '0x'"),
            16,
        )
        .expect("Failed to parse vkey hash")
        .to_bytes_be();

        assert!(SP1Verifier::verify_groth16(
            &groth16_proof,
            &vkey_hash,
            proof.public_values.as_slice()
        )
        .is_ok());
    }
}