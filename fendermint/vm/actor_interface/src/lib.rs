// Copyright 2022-2023 Protocol Labs
// SPDX-License-Identifier: Apache-2.0, MIT
//! The modules in this crate a thin interfaces to builtin-actors,
//! so that the rest of the system doesn't have to copy-paste things
//! such as actor IDs, method numbers, method parameter data types.
//!
//! This is similar to how the FVM library contains copies for actors
//! it assumes to be deployed, like the init-actor. There, it's to avoid
//! circular project dependencies. Here, we have the option to reference
//! the actor projects directly and re-export what we need, or to copy
//! the relevant pieces of code. By limiting this choice to this crate,
//! the rest of the application can avoid ad-hoc magic numbers.
//!
//! The actor IDs can be found in [singletons](https://github.com/filecoin-project/builtin-actors/blob/master/runtime/src/builtin/singletons.rs),
//! while the code IDs are in [builtins](https://github.com/filecoin-project/builtin-actors/blob/master/runtime/src/runtime/builtins.rs)

macro_rules! define_singleton {
    ($name:ident { id: $id:literal, code_id: $code_id:literal }) => {
        paste::paste! {
            pub const [<$name _ACTOR_ID>]: fvm_shared::ActorID = $id;
            pub const [<$name _ACTOR_ADDR>]: fvm_shared::address::Address = fvm_shared::address::Address::new_id([<$name _ACTOR_ID>]);
            /// Position of the actor in the builtin actor bundle manifest.
            pub const [<$name _ACTOR_CODE_ID>]: u32 = $code_id;
        }
    };
}

pub mod cron;
pub mod eam;
pub mod init;
pub mod system;
