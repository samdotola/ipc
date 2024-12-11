// Copyright 2024 Textile
// Copyright 2021-2023 Protocol Labs
// SPDX-License-Identifier: Apache-2.0, MIT

use fendermint_actor_hoku_config_shared::{HokuConfig, Method, SetAdminParams, SetConfigParams};
use fendermint_actor_machine::resolve_external;
use fil_actors_runtime::actor_error;
use fil_actors_runtime::runtime::{ActorCode, Runtime};
use fil_actors_runtime::SYSTEM_ACTOR_ADDR;
use fil_actors_runtime::{actor_dispatch, ActorError};
use fvm_ipld_encoding::tuple::*;
use fvm_shared::address::Address;
use fvm_shared::clock::ChainEpoch;

#[cfg(feature = "fil-actor")]
fil_actors_runtime::wasm_trampoline!(Actor);

pub const ACTOR_NAME: &str = "hoku_config";

#[derive(Serialize_tuple, Deserialize_tuple, Debug, Clone)]
pub struct State {
    /// The admin address that is allowed to update the config.
    pub admin: Option<Address>,
    /// The Hoku network configuration.
    pub config: HokuConfig,
}

#[derive(Serialize_tuple, Deserialize_tuple, Debug, Clone)]
pub struct ConstructorParams {
    initial_blob_capacity: u64,
    initial_blob_credits_per_byte_block: u64,
    initial_blob_credit_debit_interval: ChainEpoch,
}

pub struct Actor {}

impl Actor {
    /// Creates the actor
    pub fn constructor(rt: &impl Runtime, params: ConstructorParams) -> Result<(), ActorError> {
        rt.validate_immediate_caller_is(std::iter::once(&SYSTEM_ACTOR_ADDR))?;
        let st = State {
            admin: None,
            config: HokuConfig {
                blob_capacity: params.initial_blob_capacity,
                blob_credits_per_byte_block: params.initial_blob_credits_per_byte_block,
                blob_credit_debit_interval: params.initial_blob_credit_debit_interval,
            },
        };
        rt.create(&st)
    }

    fn set_admin(rt: &impl Runtime, params: SetAdminParams) -> Result<(), ActorError> {
        Self::ensure_update_allowed(rt)?;
        let (new_admin, _) = resolve_external(rt, params.0)?;
        rt.transaction(|st: &mut State, _rt| {
            st.admin = Some(new_admin);
            Ok(())
        })
    }

    fn get_admin(rt: &impl Runtime) -> Result<Option<Address>, ActorError> {
        rt.validate_immediate_caller_accept_any()?;
        rt.state::<State>().map(|s| s.admin)
    }

    fn set_config(rt: &impl Runtime, params: SetConfigParams) -> Result<(), ActorError> {
        let admin_exists = Self::ensure_update_allowed(rt)?;
        let new_admin = if !admin_exists {
            // The first caller becomes admin
            let (new_admin, _) = resolve_external(rt, rt.message().caller())?;
            Some(new_admin)
        } else {
            None
        };
        rt.transaction(|st: &mut State, _rt| {
            if let Some(new_admin) = new_admin {
                st.admin = Some(new_admin);
            }
            st.config = params;
            Ok(())
        })?;

        Ok(())
    }

    fn get_config(rt: &impl Runtime) -> Result<HokuConfig, ActorError> {
        rt.validate_immediate_caller_accept_any()?;
        rt.state::<State>().map(|s| s.config)
    }

    /// Ensures that immediate caller is allowed to update the config.
    /// Returns whether the admin exists.
    fn ensure_update_allowed(rt: &impl Runtime) -> Result<bool, ActorError> {
        let st = rt.state::<State>()?;
        let admin_exists = if let Some(admin) = st.admin {
            if let Some(admin_id) = rt.resolve_address(&admin) {
                rt.validate_immediate_caller_is(std::iter::once(&Address::new_id(admin_id)))?
            } else {
                // This should not happen.
                return Err(ActorError::forbidden(String::from(
                    "failed to resolve config admin id",
                )));
            }
            true
        } else {
            // The first caller becomes the admin
            rt.validate_immediate_caller_accept_any()?;
            false
        };
        Ok(admin_exists)
    }
}

impl ActorCode for Actor {
    type Methods = Method;

    fn name() -> &'static str {
        ACTOR_NAME
    }

    actor_dispatch! {
        Constructor => constructor,
        SetAdmin => set_admin,
        GetAdmin => get_admin,
        SetConfig => set_config,
        GetConfig => get_config,
    }
}

// TODO: Modify these tests for this actor
// #[cfg(test)]
// mod tests {
//     use crate::{Actor, Constants, ConstructorParams, Method, State};
//     use fendermint_actors_api::gas_market::{Reading, Utilization};
//     use fil_actors_runtime::test_utils::{expect_empty, MockRuntime, SYSTEM_ACTOR_CODE_ID};
//     use fil_actors_runtime::SYSTEM_ACTOR_ADDR;
//     use fvm_ipld_encoding::ipld_block::IpldBlock;
//     use fvm_shared::address::Address;
//     use fvm_shared::econ::TokenAmount;
//     use fvm_shared::error::ExitCode;
//
//     pub fn construct_and_verify() -> MockRuntime {
//         let rt = MockRuntime {
//             receiver: Address::new_id(10),
//             ..Default::default()
//         };
//
//         rt.set_caller(*SYSTEM_ACTOR_CODE_ID, SYSTEM_ACTOR_ADDR);
//         rt.expect_validate_caller_addr(vec![SYSTEM_ACTOR_ADDR]);
//
//         let result = rt
//             .call::<Actor>(
//                 Method::Constructor as u64,
//                 IpldBlock::serialize_cbor(&ConstructorParams {
//                     initial_base_fee: TokenAmount::from_atto(100),
//                     constants: Constants::default(),
//                 })
//                 .unwrap(),
//             )
//             .unwrap();
//         expect_empty(result);
//         rt.verify();
//         rt.reset();
//
//         rt
//     }
//
//     #[test]
//     fn test_set_ok() {
//         let rt = construct_and_verify();
//
//         rt.set_caller(*SYSTEM_ACTOR_CODE_ID, SYSTEM_ACTOR_ADDR);
//         rt.expect_validate_caller_addr(vec![SYSTEM_ACTOR_ADDR]);
//
//         let r = rt.call::<Actor>(
//             Method::SetConstants as u64,
//             IpldBlock::serialize_cbor(&Constants {
//                 minimal_base_fee: Default::default(),
//                 elasticity_multiplier: 0,
//                 base_fee_max_change_denominator: 0,
//                 block_gas_limit: 20,
//             })
//             .unwrap(),
//         );
//         assert!(r.is_ok());
//
//         let s = rt.get_state::<State>();
//         assert_eq!(s.constants.block_gas_limit, 20);
//     }
//
//     #[test]
//     fn test_update_utilization_full_usage() {
//         let rt = construct_and_verify();
//
//         rt.set_caller(*SYSTEM_ACTOR_CODE_ID, SYSTEM_ACTOR_ADDR);
//         rt.expect_validate_caller_addr(vec![SYSTEM_ACTOR_ADDR]);
//
//         let r = rt.call::<Actor>(
//             Method::UpdateUtilization as u64,
//             IpldBlock::serialize_cbor(&Utilization {
//                 // full block usage
//                 block_gas_used: 10_000_000_000,
//             })
//             .unwrap(),
//         );
//         assert!(r.is_ok());
//
//         rt.expect_validate_caller_any();
//         let r = rt
//             .call::<Actor>(Method::CurrentReading as u64, None)
//             .unwrap()
//             .unwrap();
//         let reading = r.deserialize::<Reading>().unwrap();
//         assert_eq!(reading.base_fee, TokenAmount::from_atto(112));
//     }
//
//     #[test]
//     fn test_update_utilization_equal_usage() {
//         let rt = construct_and_verify();
//
//         rt.set_caller(*SYSTEM_ACTOR_CODE_ID, SYSTEM_ACTOR_ADDR);
//         rt.expect_validate_caller_addr(vec![SYSTEM_ACTOR_ADDR]);
//
//         let r = rt.call::<Actor>(
//             Method::UpdateUtilization as u64,
//             IpldBlock::serialize_cbor(&Utilization {
//                 // full block usage
//                 block_gas_used: 5_000_000_000,
//             })
//             .unwrap(),
//         );
//         assert!(r.is_ok());
//
//         rt.expect_validate_caller_any();
//         let r = rt
//             .call::<Actor>(Method::CurrentReading as u64, None)
//             .unwrap()
//             .unwrap();
//         let reading = r.deserialize::<Reading>().unwrap();
//         assert_eq!(reading.base_fee, TokenAmount::from_atto(100));
//     }
//
//     #[test]
//     fn test_update_utilization_under_usage() {
//         let rt = construct_and_verify();
//
//         rt.set_caller(*SYSTEM_ACTOR_CODE_ID, SYSTEM_ACTOR_ADDR);
//         rt.expect_validate_caller_addr(vec![SYSTEM_ACTOR_ADDR]);
//
//         let r = rt.call::<Actor>(
//             Method::UpdateUtilization as u64,
//             IpldBlock::serialize_cbor(&Utilization {
//                 // full block usage
//                 block_gas_used: 100_000_000,
//             })
//             .unwrap(),
//         );
//         assert!(r.is_ok());
//
//         rt.expect_validate_caller_any();
//         let r = rt
//             .call::<Actor>(Method::CurrentReading as u64, None)
//             .unwrap()
//             .unwrap();
//         let reading = r.deserialize::<Reading>().unwrap();
//         assert_eq!(reading.base_fee, TokenAmount::from_atto(88));
//     }
//
//     #[test]
//     fn test_not_allowed() {
//         let rt = construct_and_verify();
//         rt.set_caller(*SYSTEM_ACTOR_CODE_ID, Address::new_id(1000));
//         rt.expect_validate_caller_addr(vec![SYSTEM_ACTOR_ADDR]);
//
//         let code = rt
//             .call::<Actor>(
//                 Method::SetConstants as u64,
//                 IpldBlock::serialize_cbor(&Constants {
//                     minimal_base_fee: TokenAmount::from_atto(10000),
//                     elasticity_multiplier: 0,
//                     base_fee_max_change_denominator: 0,
//                     block_gas_limit: 20,
//                 })
//                 .unwrap(),
//             )
//             .unwrap_err()
//             .exit_code();
//         assert_eq!(code, ExitCode::USR_FORBIDDEN)
//     }
// }
