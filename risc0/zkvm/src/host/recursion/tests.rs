// Copyright 2024 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use risc0_circuit_recursion::CircuitImpl;
use risc0_zkp::{
    adapter::CircuitInfo,
    core::digest::{Digest, DIGEST_WORDS},
    field::baby_bear::BabyBearElem,
};
use risc0_zkvm_methods::{multi_test::MultiTestSpec, MULTI_TEST_ELF, MULTI_TEST_ID};
use serial_test::serial;
use test_log::test;

use super::{
    identity_p254, join, lift, prove::poseidon254_hal_pair, prove::poseidon2_hal_pair, resolve,
    Prover, ProverOpts,
};
use crate::{
    get_prover_server, ExecutorEnv, ExecutorImpl, InnerReceipt, Receipt, SegmentReceipt, Session,
    VerifierContext,
};

// Failure on older mac minis in the lab with Intel UHD 630 graphics:
// (signal: 11, SIGSEGV: invalid memory reference)
#[cfg_attr(
    not(all(feature = "metal", target_os = "macos", target_arch = "x86_64")),
    test
)]
#[serial]
fn test_recursion() {
    use risc0_zkp::core::{digest::Digest, hash::poseidon::PoseidonHashSuite};

    let suite = PoseidonHashSuite::new_suite();
    let hal_pair = poseidon254_hal_pair();
    let (hal, circuit_hal) = (hal_pair.hal.as_ref(), hal_pair.circuit_hal.as_ref());

    // First, run the simple test of the recursion circuit.  This
    // control tree just combines two hashes.
    let digest1 = Digest::from([0, 1, 2, 3, 4, 5, 6, 7]);
    let digest2 = Digest::from([8, 9, 10, 11, 12, 13, 14, 15]);
    let expected = suite.hashfn.hash_pair(&digest1, &digest2);
    let mut prover =
        Prover::new_test_recursion_circuit([&digest1, &digest2], ProverOpts::default()).unwrap();
    let receipt = prover
        .run_with_hal(hal, circuit_hal)
        .expect("Running prover failed");

    // Uncomment to write seal...
    // let seal : Vec<u8> = bytemuck::cast_slice(receipt.seal.as_slice()).into();
    // std::fs::write("recursion.seal", seal);

    const DIGEST_SHORTS: usize = DIGEST_WORDS * 2;
    assert_eq!(CircuitImpl::OUTPUT_SIZE, DIGEST_SHORTS * 2);
    let output_elems: &[BabyBearElem] =
        bytemuck::cast_slice(&receipt.seal[..CircuitImpl::OUTPUT_SIZE]);
    let output_digest = shorts_to_digest(&output_elems[DIGEST_SHORTS..2 * DIGEST_SHORTS]);

    tracing::debug!("Receipt output: {:?}", output_digest);
    assert_eq!(output_digest, *expected);
}

// Failure on older mac minis in the lab with Intel UHD 630 graphics:
// (signal: 11, SIGSEGV: invalid memory reference)
#[cfg_attr(
    not(all(feature = "metal", target_os = "macos", target_arch = "x86_64")),
    test
)]
#[serial]
fn test_recursion_poseidon2() {
    use risc0_zkp::core::{digest::Digest, hash::poseidon::PoseidonHashSuite};

    let suite = PoseidonHashSuite::new_suite();
    let hal_pair = poseidon2_hal_pair();
    let (hal, circuit_hal) = (hal_pair.hal.as_ref(), hal_pair.circuit_hal.as_ref());

    // First, run the simple test of the recursion circuit.  This
    // control tree just combines two hashes.
    let digest1 = Digest::from([0, 1, 2, 3, 4, 5, 6, 7]);
    let digest2 = Digest::from([8, 9, 10, 11, 12, 13, 14, 15]);
    let expected = suite.hashfn.hash_pair(&digest1, &digest2);
    let mut prover =
        Prover::new_test_recursion_circuit([&digest1, &digest2], ProverOpts::default()).unwrap();

    tracing::info!("Begin");
    let receipt = prover
        .run_with_hal(hal, circuit_hal)
        .expect("Running prover failed");
    tracing::info!("End");

    // Uncomment to write seal...
    // let seal : Vec<u8> = bytemuck::cast_slice(receipt.seal.as_slice()).into();
    // std::fs::write("recursion.seal", seal);

    const DIGEST_SHORTS: usize = DIGEST_WORDS * 2;
    assert_eq!(CircuitImpl::OUTPUT_SIZE, DIGEST_SHORTS * 2);
    let output_elems: &[BabyBearElem] =
        bytemuck::cast_slice(&receipt.seal[..CircuitImpl::OUTPUT_SIZE]);
    let output_digest = shorts_to_digest(&output_elems[DIGEST_SHORTS..2 * DIGEST_SHORTS]);

    tracing::debug!("Receipt output: {:?}", output_digest);
    assert_eq!(output_digest, *expected);
}

fn shorts_to_digest(elems: &[BabyBearElem]) -> Digest {
    let words: Vec<u32> = elems
        .chunks_exact(2)
        .map(|shortpair| {
            let [a, b] = shortpair else { unreachable!() };
            ((u64::from(*b) << 16) + u64::from(*a)) as u32
        })
        .collect();
    Digest::try_from(words.as_slice()).unwrap()
}

fn generate_busy_loop_segments(hashfn: &str) -> (Session, Vec<SegmentReceipt>) {
    let segment_limit_po2 = 16; // 64k cycles
    let cycles = 1 << segment_limit_po2;
    let env = ExecutorEnv::builder()
        .write(&MultiTestSpec::BusyLoop { cycles })
        .unwrap()
        .segment_limit_po2(segment_limit_po2)
        .build()
        .unwrap();

    tracing::info!("Executing rv32im");
    let mut exec = ExecutorImpl::from_elf(env, MULTI_TEST_ELF).unwrap();
    let session = exec.run().unwrap();
    let segments = session.resolve().unwrap();
    tracing::info!("Got {} segments", segments.len());

    let opts = crate::ProverOpts {
        hashfn: hashfn.to_string(),
        prove_guest_errors: false,
    };
    let prover = get_prover_server(&opts).unwrap();

    tracing::info!("Proving rv32im");
    let ctx = VerifierContext::default();
    let segment_receipts = segments
        .iter()
        .map(|x| prover.prove_segment(&ctx, x).unwrap())
        .collect();
    tracing::info!("Done proving rv32im");

    (session, segment_receipts)
}

#[cfg_attr(
    not(all(feature = "metal", target_os = "macos", target_arch = "x86_64")),
    test
)]
#[serial]
fn test_recursion_lift_join_identity_e2e() {
    // Prove the base case
    let (session, segments) = generate_busy_loop_segments("poseidon");

    // Lift and join them  all (and verify)
    let mut rollup = lift(&segments[0]).unwrap();
    tracing::info!("Lift claim = {:?}", rollup.claim);
    let ctx = VerifierContext::default();
    for receipt in &segments[1..] {
        let rec_receipt = lift(receipt).unwrap();
        tracing::info!("Lift claim = {:?}", rec_receipt.claim);
        rec_receipt.verify_integrity_with_context(&ctx).unwrap();
        rollup = join(&rollup, &rec_receipt).unwrap();
        tracing::info!("Join claim = {:?}", rollup.claim);
        rollup.verify_integrity_with_context(&ctx).unwrap();
    }

    // Check on stark-to-snark
    // let snark_receipt =
    identity_p254(&rollup).expect("Running prover failed");

    // Uncomment to write seal...
    // let seal: Vec<u8> =
    // bytemuck::cast_slice(snark_receipt.seal.as_slice()).into();
    // std::fs::write("recursion.seal", seal);

    // Validate the Session rollup + journal data
    let rollup_receipt = Receipt::new(
        InnerReceipt::Succinct(rollup),
        session.journal.unwrap().bytes,
    );
    rollup_receipt.verify(MULTI_TEST_ID).unwrap();
}

fn generate_composition_receipt(hashfn: &str) -> Receipt {
    let opts = crate::ProverOpts {
        hashfn: hashfn.to_string(),
        prove_guest_errors: false,
    };
    let prover = get_prover_server(&opts).unwrap();

    tracing::info!("Proving rv32im: echo 'execution A'");
    let env = ExecutorEnv::builder()
        .write(&MultiTestSpec::Echo {
            bytes: b"execution A".to_vec(),
        })
        .unwrap()
        .build()
        .unwrap();
    let assumption_receipt_a = prover.prove(env, MULTI_TEST_ELF).unwrap();
    tracing::info!("Done proving rv32im: echo 'execution A'");

    tracing::info!("Proving rv32im: echo 'execution B'");
    let env = ExecutorEnv::builder()
        .write(&MultiTestSpec::Echo {
            bytes: b"execution B".to_vec(),
        })
        .unwrap()
        .build()
        .unwrap();
    let assumption_receipt_b = prover.prove(env, MULTI_TEST_ELF).unwrap();
    tracing::info!("Done proving rv32im: echo 'execution B'");

    let env = ExecutorEnv::builder()
        .add_assumption(assumption_receipt_a.clone().into())
        .add_assumption(assumption_receipt_b.clone().into())
        .write(&MultiTestSpec::SysVerify(vec![
            (MULTI_TEST_ID.into(), b"execution A".to_vec()),
            (MULTI_TEST_ID.into(), b"execution B".to_vec()),
        ]))
        .unwrap()
        .build()
        .unwrap();

    tracing::info!("Proving rv32im: sys_verify");
    let composition_receipt = prover.prove(env, MULTI_TEST_ELF).unwrap();
    tracing::info!("Done proving rv32im: sys_verify");

    composition_receipt
}

#[cfg_attr(
    not(all(feature = "metal", target_os = "macos", target_arch = "x86_64")),
    test
)]
#[serial]
fn test_recursion_lift_resolve_e2e() {
    let receipt = generate_composition_receipt("poseidon");
    let composition_receipt = receipt.inner.composite().unwrap().clone();
    assert_eq!(composition_receipt.segments.len(), 1);
    let conditional_segment_receipt = composition_receipt.segments[0].clone();

    assert_eq!(composition_receipt.assumptions.len(), 2);
    let lifted_assumptions = composition_receipt
        .assumptions
        .iter()
        .map(|receipt| {
            let assumption_receipt = receipt.composite().unwrap().clone();
            assert_eq!(assumption_receipt.segments.len(), 1);
            assert_eq!(assumption_receipt.assumptions.len(), 0);
            let assumption_segment_receipt = assumption_receipt.segments[0].clone();

            // Lift the assumption receipt.
            tracing::info!("Lifting assumption");
            let lifted_assumption = lift(&assumption_segment_receipt).unwrap();
            lifted_assumption
                .verify_integrity_with_context(&VerifierContext::default())
                .unwrap();
            tracing::info!("Lift assumption claim = {:?}", lifted_assumption.claim);

            lifted_assumption
        })
        .collect::<Vec<_>>();

    tracing::info!("Lifting conditional");
    let lifted_conditional = lift(&conditional_segment_receipt).unwrap();
    lifted_conditional
        .verify_integrity_with_context(&VerifierContext::default())
        .unwrap();
    tracing::info!("Lift conditional claim = {:?}", lifted_conditional.claim);

    let resolved =
        lifted_assumptions
            .into_iter()
            .fold(lifted_conditional, |conditional, assumption| {
                tracing::info!("Resolve");
                let resolved = resolve(&conditional, &assumption).unwrap();
                resolved
                    .verify_integrity_with_context(&VerifierContext::default())
                    .unwrap();
                tracing::info!("Resolve claim = {:?}", resolved.claim);

                resolved
            });

    // Validate the Session rollup + journal data
    let resolved_receipt = Receipt::new(InnerReceipt::Succinct(resolved), receipt.journal.bytes);
    resolved_receipt.verify(MULTI_TEST_ID).unwrap();
}
