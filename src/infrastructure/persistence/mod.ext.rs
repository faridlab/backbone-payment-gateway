// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

mod gateway_transaction_repository_custom;
pub use gateway_transaction_repository_custom::{
    compose_fee_post, compose_fee_reversal, FeeSourceRow, SettledHeaderRow,
};
